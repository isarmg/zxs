//! Generation-owned fences, independent archives, pending joint authority and
//! explicit Add/Abort/Exit activation. A fence or portable control certificate
//! alone never grants active write or disk lifecycle completion authority.
use super::{Engine, ValidatedConfig, recovery_checkpoint::Records};
use crate::{disk::*, journal::*, publication_evidence::verified_object_at, *};
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod abort;
mod archive;
pub(in crate::engine) use archive::PreparedBackupControl;
mod backup;
pub(crate) use backup::PreparedBackupFirstCandidate;
pub(in crate::engine) use backup::{
    AdmittedBackupDecision, BackupChoiceContext, BackupFrozenControl, PreparedBackupAuthority,
    PreparedBackupFirstPublication,
};
pub use backup::{
    BackupFirstFinalizeReport, BackupFirstPublicationHash, BackupFirstPublicationReport,
    BackupRegistryDecisionHash,
};
mod complete_chain;
pub use abort::MembershipAbortStatus;
mod decision;
mod effect;
pub(in crate::engine) mod readmission;
pub use readmission::{DiskReadmissionPhase, DiskReadmissionStatus};
pub(in crate::engine) mod terminal;
pub(in crate::engine) mod terminal_chain;
pub(in crate::engine) mod terminal_resume;
pub use effect::MembershipExitAction;
use effect::{ExitEffect, validate_transition_target};
pub use pending::activation::exit::{DiskExitPhase, DiskExitStatus};
pub use pending::activation::terminal_resume::{
    TerminalMetadataResumePhase, TerminalMetadataResumeRecovery,
};
pub use pending::terminal_resume::{
    TerminalMetadataResume, TerminalMetadataResumeCancelOutcome,
    TerminalMetadataResumeCancellationPhase, TerminalMetadataResumeStatus,
};
pub(crate) use terminal_chain::FirstSlotOwner;
pub use terminal_chain::{
    TerminalChainActivationPlanReport, TerminalChainFirstPublication,
    TerminalChainGenerationLogReport, TerminalChainGenerationSelectionReport,
    TerminalChainIoDecisionReport, TerminalChainPublishedCurrent,
    TerminalChainPublishedCurrentReport, TerminalChainReadmissionActivation,
    TerminalChainReadmissionPhase, TerminalChainReadmissionStatus, TerminalChainSelectedGeneration,
};
pub use terminal_resume::{TerminalMetadataResumeAdmission, TerminalReadmissionPreview};
pub(crate) mod pending;
pub use archive::{
    MembershipArchiveInspection, MembershipControlArchive, MembershipJournalInspection,
    verify_membership_control_archive,
};
pub use pending::{
    MembershipJointDecisionStatus, MembershipJointRole, MembershipJointVote,
    PendingMemberControlReceipt, PendingMemberDirectory, PendingMembershipStatus,
};

const INTENT_KIND: &str = "membership-transition";

fn changes_registered_membership(config: &ValidatedConfig, target: &ValidatedConfig) -> bool {
    !config.same_metadata_membership(target)
        || target
            .disks
            .iter()
            .any(|disk| !config.disks.iter().any(|old| old.disk_id == disk.disk_id))
}

/// Genesis and a completed predecessor are distinct authority origins.
/// A successor must carry the exact predecessor hash; it cannot restart Genesis.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(tag = "kind", deny_unknown_fields)]
enum Origin {
    #[serialization(rename = "unified-genesis-v3")]
    UnifiedGenesis {
        config: ValidatedConfig,
        evidence: Box<crate::engine::snapshot_initialization::bootstrap::successor::UnifiedOriginEvidence>,
    },
    #[serialization(rename = "unified-successor-v3")]
    UnifiedSuccessor {
        config: ValidatedConfig,
        evidence: Box<crate::engine::snapshot_initialization::bootstrap::successor::UnifiedOriginEvidence>,
        predecessor_authority_hash: String,
    },
    Genesis {
        config: ValidatedConfig,
    },
    Successor {
        config: ValidatedConfig,
        predecessor_authority_hash: String,
    },
    BackupCutover {
        config: ValidatedConfig,
        evidence: Box<backup::Evidence>,
    },
    Readmission {
        config: ValidatedConfig,
        predecessor_authority_hash: String,
        request: Box<readmission::ReadmissionRequest>,
    },
    TerminalResume {
        config: ValidatedConfig,
        predecessor_authority_hash: Option<String>,
        terminal_authority_hash: String,
    },
}
impl Origin {
    fn retained_seal_format(&self) -> Result<DiskSealFormat> {
        match self {
            Self::UnifiedGenesis { evidence, .. } => return evidence.original_seal_format(),
            Self::UnifiedSuccessor { evidence, predecessor_authority_hash, .. } => {
                return evidence.successor_seal_format(predecessor_authority_hash);
            }
            _ => {}
        }
        Ok(match self.predecessor() {
            Some(hash) => DiskSealFormat::MembershipGeneration {
                authority_hash: hash.to_owned(),
            },
            None => DiskSealFormat::ManagedDirectory,
        })
    }

    fn config(&self) -> &ValidatedConfig {
        match self {
            Self::UnifiedGenesis { config, .. }
            | Self::UnifiedSuccessor { config, .. }
            | Self::Genesis { config }
            | Self::Successor { config, .. }
            | Self::TerminalResume { config, .. }
            | Self::Readmission { config, .. }
            | Self::BackupCutover { config, .. } => config,
        }
    }
    fn predecessor(&self) -> Option<&str> {
        match self {
            Self::UnifiedGenesis { .. } | Self::Genesis { .. } | Self::BackupCutover { .. } => None,
            Self::UnifiedSuccessor { predecessor_authority_hash, .. }
            | Self::Successor {
                predecessor_authority_hash,
                ..
            }
            | Self::Readmission {
                predecessor_authority_hash,
                ..
            } => Some(predecessor_authority_hash),
            Self::TerminalResume {
                predecessor_authority_hash,
                ..
            } => predecessor_authority_hash.as_deref(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct PublishedBoundary {
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    candidate_hash: String,
    snapshot_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct TransitionIntent {
    operation_id: OperationId,
    principal: String,
    origin: Origin,
    target: ValidatedConfig,
    added_roots: BTreeMap<String, pending::MemberIdentity>,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    effect: Option<ExitEffect>,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    foreign: Option<crate::foreign_import::ForeignAdmissionRecipe>,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    fence_scope: Option<FenceScope>,
    boundary: Option<PublishedBoundary>,
    #[serialization(with = "storage_contract::u64_string")]
    fence_sequence: u64,
}

/// An explicit, hash-bound proof scope, never a reinterpretation of an old
/// all-member intent. Only the departing old-only voter may be absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum FenceScope {
    #[serialization(rename = "retained-exit-quorum")]
    RetainedExitQuorum,
}

impl TransitionIntent {
    fn first_sequence(&self) -> Result<u64> {
        match &self.foreign {
            Some(recipe) => recipe.first_sequence(self.fence_sequence),
            None => self.fence_sequence.checked_add(1).ok_or_else(|| {
                Error::new(ErrorCode::ResourceLimit, "activation sequence exhausted")
            }),
        }
    }
    fn verify_target(&self) -> Result<()> {
        match &self.origin {
            Origin::UnifiedGenesis { config, evidence } => evidence.require_genesis(config)?,
            Origin::UnifiedSuccessor { config, evidence, .. } => evidence.require_pool(config)?,
            _ => {}
        }
        if let Some(recipe) = &self.foreign {
            if self.effect.is_some()
                || self.fence_scope.is_some()
                || !matches!(
                    self.origin,
                    Origin::Genesis { .. } | Origin::Successor { .. }
                )
            {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "foreign recipe cannot reinterpret another membership protocol",
                ));
            }
            recipe.verify_contract(self.origin.config(), &self.target, self.operation_id)?;
        }
        match &self.origin {
            Origin::Readmission { request, .. } => request.verify_target(self),
            Origin::BackupCutover { .. } => backup::verify_intent(self),
            _ => {
                validate_transition_target(self.origin.config(), &self.target, self.effect.as_ref())
            }
        }
    }

    fn verify_fence_scope(&self) -> Result<()> {
        if self.fence_scope.is_none() {
            return Ok(());
        }
        let original = self.origin.config();
        let effect = self.effect.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::ConfigConflict,
                "quorum fence requires an exit effect",
            )
        })?;
        effect.validate(original, &self.target)?;
        if self.boundary.is_none()
            || self.old_only().count() != 1
            || !self.old_only().any(|disk| disk.disk_id == effect.disk_id)
            || self.target.metadata_members().len() < original.metadata_quorum()
        {
            return Err(Error::new(
                ErrorCode::MetadataQuorumUnavailable,
                "retained exit fence requires a published source and the unchanged original quorum",
            ));
        }
        Ok(())
    }

    fn fence_members(&self) -> Vec<DiskConfig> {
        let config = if self.fence_scope.is_some() {
            &self.target
        } else {
            self.origin.config()
        };
        let mut members = config.metadata_members().cloned().collect::<Vec<_>>();
        members.sort_by(|left, right| left.disk_id.cmp(&right.disk_id));
        members
    }

    /// Rebind paths only after the caller's original membership is verified.
    /// Persisted intent identities and the fixed witness set stay unchanged.
    fn fence_members_at(&self, config: &ValidatedConfig) -> Result<Vec<DiskConfig>> {
        config.verify_saved_metadata_membership(self.origin.config())?;
        self.fence_members()
            .into_iter()
            .map(|mut disk| {
                let current = config
                    .metadata_members()
                    .find(|current| current.disk_id == disk.disk_id)
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::ConfigConflict,
                            "fence witness is absent from the admitted original membership",
                        )
                    })?;
                disk.root = current.root.clone();
                Ok(disk)
            })
            .collect()
    }

    fn participants(&self) -> Vec<DiskConfig> {
        let mut members = self
            .target
            .metadata_members()
            .map(|disk| (disk.disk_id.clone(), disk.clone()))
            .collect::<BTreeMap<_, _>>();
        for disk in self
            .origin
            .config()
            .metadata_members()
            .filter(|_| self.fence_scope.is_none())
        {
            members
                .entry(disk.disk_id.clone())
                .or_insert_with(|| disk.clone());
        }
        members.into_values().collect()
    }
    fn old_only(&self) -> impl Iterator<Item = &DiskConfig> {
        self.origin.config().metadata_members().filter(|old| {
            !self
                .target
                .metadata_members()
                .any(|target| target.disk_id == old.disk_id)
        })
    }
}

#[derive(Clone)]
pub(super) struct AdmittedFence {
    intent: TransitionIntent,
    hash: String,
    receipts: BTreeMap<String, Receipt>,
    prepared_roots: BTreeMap<String, pending::reservation::ReservedRoot>,
}

/// Observed, synchronized old-member fence receipts. Neither a quorum nor all
/// receipts authorizes the target membership to write or retire any old disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembershipFenceStatus {
    pub operation_id: OperationId,
    pub transition_hash: String,
    pub confirmed_members: usize,
    pub original_members: usize,
    pub original_quorum: usize,
}
impl MembershipFenceStatus {
    pub fn quorum_fenced(&self) -> bool {
        self.confirmed_members >= self.original_quorum
    }
    pub fn all_members_fenced(&self) -> bool {
        self.confirmed_members == self.original_members
    }
}

impl Engine {
    pub fn membership_fence_status(&self) -> Option<MembershipFenceStatus> {
        self.membership_fence
            .as_ref()
            .map(|fence| MembershipFenceStatus {
                operation_id: fence.intent.operation_id,
                transition_hash: fence.hash.clone(),
                confirmed_members: fence.receipts.len(),
                original_members: self.config.metadata_members().len(),
                original_quorum: self.config.metadata_quorum(),
            })
    }

    pub(crate) fn require_unfenced_membership(&self) -> Result<()> {
        if self.membership_fence.is_some() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "membership handoff has started or may have persisted; original writes remain blocked",
            ));
        }
        Ok(())
    }

    /// Experimental first barrier, not an add/remove operation. It permanently
    /// pauses that generation's business decisions. Pure-add activation is explicit;
    /// certified cancellation is explicit; removal uses its separate lifecycle.
    /// Every added directory is reserved and writer-locked before the old fence.
    /// Partial preparation is resumable only by the same operation and exact roots.
    /// A failed append may have persisted: inspect/reopen and retry the same intent.
    pub fn begin_membership_handoff(
        &mut self,
        target: Config,
        principal: &str,
        operation_id: OperationId,
    ) -> Result<MembershipFenceStatus> {
        let target = ValidatedConfig::try_from(target)?;
        pending::admission::validate_pure_add_target(&self.config, &target, &self.root)?;
        self.require_addition_chain(&target)?;
        self.begin_membership_handoff_with_effect(target, None, principal, operation_id)
    }

    /// Bind an authorized caller's exact directory handles before any source
    /// preparation, new-root reservation or old-generation fence is written.
    /// The supplied set must contain every added directory exactly once.
    pub fn begin_membership_handoff_with_directories(
        &mut self,
        target: Config,
        principal: &str,
        operation_id: OperationId,
        directories: Vec<PendingMemberDirectory>,
    ) -> Result<MembershipFenceStatus> {
        let target = ValidatedConfig::try_from(target)?;
        pending::admission::validate_pure_add_target(&self.config, &target, &self.root)?;
        self.require_addition_chain(&target)?;
        self.begin_membership_handoff_prepared(
            target,
            None,
            None,
            principal,
            operation_id,
            Some(directories),
        )
    }

    /// Begin one foreign-chain operation from its admitted native source, not
    /// an editable mapped State or saved proposal. Root enrollment and physical
    /// activation remain fail-closed until their foreign-preserving path exists.
    /// No generic empty-root admission rule is relaxed by this entry.
    pub fn begin_foreign_membership_handoff(
        &mut self,
        request: ForeignIdentityTargetRequest,
        source: &ForeignImportPreview,
        principal: &str,
        directories: Vec<PendingMemberDirectory>,
    ) -> Result<MembershipFenceStatus> {
        let operation = request.operation_id;
        let target = ValidatedConfig::try_from(request.proposed_configuration.clone())?;
        let recipe = match self
            .membership_fence
            .as_ref()
            .and_then(|fence| fence.intent.foreign.as_ref())
        {
            Some(recipe) => {
                recipe.recheck_request_source(&request, source)?;
                recipe.clone()
            }
            None => {
                let names = self.capture_foreign_identity_target(request)?;
                self.capture_foreign_admission_recipe(&names, source)?
            }
        };
        self.require_addition_chain(&target)?;
        self.begin_membership_handoff_prepared(
            target,
            None,
            Some(recipe),
            principal,
            operation,
            Some(directories),
        )
    }

    fn begin_membership_handoff_with_effect(
        &mut self,
        target: ValidatedConfig,
        effect: Option<ExitEffect>,
        principal: &str,
        operation_id: OperationId,
    ) -> Result<MembershipFenceStatus> {
        self.begin_membership_handoff_prepared(target, effect, None, principal, operation_id, None)
    }

    fn begin_membership_handoff_prepared(
        &mut self,
        target: ValidatedConfig,
        effect: Option<ExitEffect>,
        foreign: Option<crate::foreign_import::ForeignAdmissionRecipe>,
        principal: &str,
        operation_id: OperationId,
        directories: Option<Vec<PendingMemberDirectory>>,
    ) -> Result<MembershipFenceStatus> {
        Self::require_actor(principal)?;
        self.require_no_commit_preparation()?;
        if let Some(generation) = self.config.generation()
            && let Some(registry) = generation.registry()
            && self.current_namespaces.as_ref().is_none_or(|view| {
                view.registry() != registry
                    || view.activation_sequence()
                        != generation
                            .first_sequence()
                            .expect("invariant: admitted generation sequence")
            })
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "a registry successor requires the admitted current namespace source",
            ));
        }
        validate_transition_target(&self.config, &target, effect.as_ref())?;
        if let Some(recipe) = &foreign {
            recipe.verify_contract(&self.config, &target, operation_id)?;
            // Recompute the complete target before any root reservation or fence.
            // This does not claim that its FULLs or databases are installed.
            let sequence = self
                .membership_fence
                .as_ref()
                .map_or(self.next_slot, |fence| fence.intent.fence_sequence);
            recipe.joint_state(&self.state, &self.config, &target, sequence)?;
        }
        let held = directories
            .map(|directories| self.admit_added_directories(&target, directories))
            .transpose()?;
        if let Some(fence) = &self.membership_fence {
            if fence.intent.operation_id != operation_id
                || fence.intent.principal != principal
                || fence.intent.effect != effect
                || fence
                    .intent
                    .foreign
                    .as_ref()
                    .map(|recipe| recipe.digest())
                    .transpose()?
                    != foreign.as_ref().map(|recipe| recipe.digest()).transpose()?
                || object_hash("membership-target", &fence.intent.target)?
                    != object_hash("membership-target", &target)?
            {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "the pending membership handoff has a different actor, operation or target",
                ));
            }
            if let Some(held) = &held {
                for (id, root) in held {
                    let disk = target
                        .disks
                        .iter()
                        .find(|disk| disk.disk_id == *id)
                        .expect("invariant: admitted added directory belongs to target");
                    let actual = pending::admission::member_identity(
                        &target,
                        disk,
                        pending::admission::verify_named_root(root)?,
                    );
                    if fence.intent.added_roots.get(id) != Some(&actual) {
                        return Err(Error::new(
                            ErrorCode::DiskIdentityMismatch,
                            "held directory differs from the durable membership intent",
                        ));
                    }
                }
            }
        } else {
            let fence_scope = effect.as_ref().and_then(|effect| {
                self.config
                    .metadata_members()
                    .find(|disk| {
                        disk.disk_id == effect.disk_id
                            && self.disk_resources.available(disk).is_none()
                    })
                    .map(|_| FenceScope::RetainedExitQuorum)
            });
            if fence_scope.is_none() {
                self.require_write()?;
            }
            if self.state.operations.contains_key(&operation_id) {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "membership operation already belongs to a published business decision",
                ));
            }
            let next_slot = self.next_slot.checked_add(1).ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "membership fence sequence exhausted",
                )
            })?;
            let origin = match self.config.generation_hash() {
                Some(hash) if self.config.unified_successor().is_some() => Origin::UnifiedSuccessor {
                    config: self.config.clone(),
                    evidence: Box::new(self.capture_unified_origin()?),
                    predecessor_authority_hash: hash.to_owned(),
                },
                Some(hash) => Origin::Successor {
                    config: self.config.clone(),
                    predecessor_authority_hash: hash.to_owned(),
                },
                None if self.config.fresh_bootstrap().is_some() => Origin::UnifiedGenesis {
                    config: self.config.clone(),
                    evidence: Box::new(self.capture_unified_origin()?),
                },
                None => Origin::Genesis {
                    config: self.config.clone(),
                },
            };
            let members = if fence_scope.is_some() {
                target.metadata_members().cloned().collect::<Vec<_>>()
            } else {
                self.members()
            };
            self.validate_fence_members(&members)?;
            if self.journals.values().any(JournalWriter::needs_recovery) {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "a new membership fence requires clean original journals",
                ));
            }
            let prepared_roots =
                self.prepare_added_roots(&origin, &target, principal, operation_id, held)?;
            let boundary = self.handoff_boundary(&members)?;
            let intent = TransitionIntent {
                operation_id,
                principal: principal.to_owned(),
                origin,
                added_roots: prepared_roots
                    .iter()
                    .map(|(id, root)| (id.clone(), root.identity.clone()))
                    .collect(),
                target,
                effect,
                foreign,
                fence_scope,
                boundary,
                fence_sequence: self.next_slot,
            };
            intent.verify_target()?;
            intent.verify_fence_scope()?;
            if intent.fence_scope.is_some() {
                // Missing-voter recovery is normally write-blocked. This exact
                // read-only prefix/decision closure is the narrower admission,
                // not permission to ignore an unresolved accepted operation.
                self.inspect_retained_exit_prefix(&intent)?;
            }
            let prepared = PreparedMetadataObject::new(INTENT_KIND, &intent, MAX_METADATA)?;
            for root in prepared_roots.values() {
                root.verify_unsealed()?;
            }
            self.blocked = true;
            self.metadata_integrity.invalidate();
            self.published_state_base = None;
            self.next_slot = next_slot;
            self.membership_fence = Some(AdmittedFence {
                intent,
                hash: prepared.hash,
                receipts: BTreeMap::new(),
                prepared_roots,
            });
        }
        self.persist_membership_fence()
    }

    fn validate_fence_members(&self, members: &[DiskConfig]) -> Result<()> {
        for disk in members {
            super::provenance::validate_owned_seal(
                &self.disk_resources,
                disk,
                &self.metadata_integrity,
            )?;
            self.config
                .verify_root_membership(&self.disk_resources.get(disk)?.root, disk)?;
            if self
                .journals
                .get(&disk.disk_id)
                .is_none_or(JournalWriter::requires_reopen)
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "membership fence needs every original journal at a clean boundary",
                ));
            }
        }
        Ok(())
    }

    fn handoff_boundary(&self, members: &[DiskConfig]) -> Result<Option<PublishedBoundary>> {
        let Some(snapshot) = &self.snapshot else {
            return Ok(None);
        };
        let disk = members.first().expect("invariant: nonempty membership");
        let scan = scan_configured_at(&self.disk_resources.get(disk)?.root, disk, &self.config)?;
        let record = scan
            .records
            .iter()
            .find(|(record, _)| {
                record.sequence == self.state.sequence
                    && record.record_type == RecordType::Publication
            })
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "published handoff prefix has no original publication record",
                )
            })?;
        let candidate: Candidate = verified_object_at(
            &self.disk_resources,
            members,
            "candidate",
            &record.0.candidate_hash,
        )?;
        self.verify_candidate_membership(&candidate)?;
        if candidate.sequence != self.state.sequence || &candidate.snapshot_hash != snapshot {
            return Err(Error::new(
                ErrorCode::LogFork,
                "handoff prefix differs from visible state",
            ));
        }
        Ok(Some(PublishedBoundary {
            sequence: candidate.sequence,
            candidate_hash: record.0.candidate_hash.clone(),
            snapshot_hash: snapshot.clone(),
        }))
    }

    fn persist_membership_fence(&mut self) -> Result<MembershipFenceStatus> {
        let members = self
            .membership_fence
            .as_ref()
            .expect("invariant: handoff is admitted")
            .intent
            .fence_members_at(&self.config)?;
        self.validate_fence_members(&members)?;
        let fence = self
            .membership_fence
            .as_ref()
            .expect("invariant: handoff is admitted");
        let sequence = fence.intent.fence_sequence;
        let hash = fence.hash.clone();
        // Reinstall the exact intent before extending a partial fence. This does
        // not repair, relabel or replace any existing Candidate or receipt.
        PreparedMetadataObject::new(INTENT_KIND, &fence.intent, MAX_METADATA)?
            .store_at(&self.disk_resources, &members)?;
        for disk in &members {
            let journal = self
                .journals
                .get_mut(&disk.disk_id)
                .expect("invariant: fence writer admission requires every journal");
            // Only the already admitted same-intent resume repairs a torn tail.
            // No ordinary candidate completion runs on this path.
            journal.repair_torn_tail()?;
            let receipt =
                journal.append(disk, RecordType::MembershipFence, sequence, &hash, &hash)?;
            self.membership_fence
                .as_mut()
                .expect("invariant: handoff is admitted")
                .receipts
                .insert(disk.disk_id.clone(), receipt);
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!(
                "after_membership_fence_{}",
                self.membership_fence
                    .as_ref()
                    .expect("invariant: handoff is admitted")
                    .receipts
                    .len()
            ))?;
        }
        Ok(self
            .membership_fence_status()
            .expect("invariant: handoff is admitted"))
    }

    pub(super) fn admit_membership_fence(
        &mut self,
        online: &[DiskConfig],
        records: &Records,
    ) -> Result<()> {
        let fences = records
            .values()
            .flat_map(|slot| slot.values())
            .flatten()
            .filter(|receipt| receipt.record.record_type == RecordType::MembershipFence)
            .collect::<Vec<_>>();
        let Some(first) = fences.first() else {
            return Ok(());
        };
        let hash = &first.record.object_hash;
        let intent: TransitionIntent =
            verified_object_at(&self.disk_resources, online, INTENT_KIND, hash).map_err(
                |error| {
                    if self.config.generation().is_some() {
                        error.context(
                            ErrorCode::RecoveryRequired,
                            "later fence requires its exact transition intent",
                        )
                    } else {
                        error
                    }
                },
            )?;
        let config = intent.origin.config();
        if intent.origin.predecessor() != self.config.generation_hash() {
            return Err(Error::new(
                ErrorCode::LogFork,
                "fence belongs to another membership generation",
            ));
        }
        self.config.verify_saved_metadata_membership(config)?;
        intent.verify_target()?;
        pending::reservation::verify_intent_roots(&intent)?;
        Self::require_actor(&intent.principal)?;
        let boundary_sequence = intent.boundary.as_ref().map_or(0, |base| base.sequence);
        if intent.fence_sequence <= boundary_sequence || intent.fence_sequence == u64::MAX {
            return Err(Error::new(
                ErrorCode::LogFork,
                "membership fence does not follow its published prefix",
            ));
        }
        let mut receipts = BTreeMap::new();
        let members = intent.fence_members();
        for receipt in fences {
            if !members
                .iter()
                .any(|disk| disk.disk_id == receipt.disk_id && disk.slot_id == receipt.slot_id)
                || !receipt_binds(
                    receipt,
                    RecordType::MembershipFence,
                    intent.fence_sequence,
                    hash,
                    hash,
                )?
            {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "conflicting membership fences",
                ));
            }
            if receipts
                .insert(receipt.disk_id.clone(), receipt.clone())
                .is_some()
            {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "duplicate membership fence in one journal",
                ));
            }
        }
        for (sequence, candidates) in records {
            for (candidate_hash, entries) in candidates {
                for receipt in entries {
                    if receipt.record.record_type == RecordType::MembershipFence {
                        continue;
                    }
                    if *sequence >= intent.fence_sequence
                        || (*sequence > boundary_sequence
                            && matches!(
                                receipt.record.record_type,
                                RecordType::Accept
                                    | RecordType::Finalize
                                    | RecordType::Publication
                                    | RecordType::Reprotect
                            ))
                        || (*sequence == boundary_sequence
                            && intent
                                .boundary
                                .as_ref()
                                .is_some_and(|base| candidate_hash != &base.candidate_hash))
                    {
                        return Err(Error::new(
                            ErrorCode::LogFork,
                            "business decision conflicts with the frozen membership prefix",
                        ));
                    }
                }
            }
        }
        self.blocked = true;
        self.membership_fence = Some(AdmittedFence {
            intent,
            hash: hash.clone(),
            receipts,
            prepared_roots: BTreeMap::new(),
        });
        Ok(())
    }

    pub(super) fn verify_membership_fence_prefix(
        &self,
        sequence: Option<u64>,
        snapshot: Option<&str>,
    ) -> Result<()> {
        let Some(fence) = &self.membership_fence else {
            return Ok(());
        };
        let expected = fence.intent.boundary.as_ref();
        if sequence != expected.map(|base| base.sequence)
            || snapshot != expected.map(|base| base.snapshot_hash.as_str())
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "membership fence requires its exact fully published historical prefix",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/membership_transition_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/membership_missing_exit_tests.rs"]
mod missing_exit_tests;
