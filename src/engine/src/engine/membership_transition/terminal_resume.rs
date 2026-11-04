//! A terminal handoff is not Genesis and has no synthetic empty old quorum.
//! The complete prior fixed-membership fence authorizes only management. An
//! explicit, independently persisted fresh-member decision authorizes resumption.
use super::archive::persistent::{
    ArchiveRoot, PendingControlArchive, abort_view::PortableAbortArchive,
};
use super::decision::DecisionReceipt;
use super::pending::{MemberIdentity, admission};
use super::terminal::{TerminalAuthority, VerifiedTerminalAuthority};
use super::*;
use crate::engine::disk_registry::{
    DiskRegistryCommitment, VerifiedDiskRegistry, published::continuation::RegistryBoundary,
};
use std::{collections::BTreeSet, sync::Arc};

pub(in crate::engine::membership_transition) const PROPOSAL_KIND: &str = "terminal-resume-proposal";
const ACCEPTANCE_KIND: &str = "terminal-resume-acceptance";

#[derive(Clone, Debug, Serialize, Deserialize)]
enum ResumeFormat {
    #[serialization(rename = "terminal-resume-generation")]
    TerminalResume,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct GraphObject {
    kind: String,
    value: json_codec::Value,
}

/// A proposal names the old archive without modifying it. The old terminal
/// intent and the new resumption intent remain distinct hash domains.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine::membership_transition) struct ResumeProposal {
    format: ResumeFormat,
    pub(super) terminal: TerminalAuthority,
    pub(super) archive: ArchiveRoot,
    old_members: PortableAbortArchive,
    pub(super) intent: TransitionIntent,
    registry: DiskRegistryCommitment,
    #[serialization(with = "storage_contract::u64_string")]
    source_business_generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    source_topology_generation: u64,
    objects: BTreeMap<String, GraphObject>,
}

/// Every configured target has one immutable vote. Requiring full installation
/// is stronger than the independent target Q, and keeps one deterministic core
/// rather than changing G when another equivalent quorum becomes available.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct ResumeCore {
    format: ResumeFormat,
    pub(super) proposal: ResumeProposal,
    votes: BTreeMap<String, DecisionReceipt>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Acceptance {
    proposal_hash: String,
    terminal_authority_hash: String,
    archive_root_hash: String,
    identity: MemberIdentity,
}

impl ResumeProposal {
    pub(super) fn prepare(
        verified: &VerifiedTerminalAuthority,
        archive: &PendingControlArchive,
        intent: TransitionIntent,
    ) -> Result<Self> {
        let terminal = &verified.authority;
        let boundary = terminal
            .intent()
            .boundary
            .as_ref()
            .expect("invariant: verified terminal boundary");
        let prepared = verified.registry.prepare_resume(
            &terminal.hash()?,
            &verified.state,
            &boundary.snapshot_hash,
            &intent.target,
        )?;
        let (_, old_members) = archive.abort_material()?;
        let objects = prepared
            .objects()
            .map(|(kind, hash, bytes)| {
                Ok((
                    hash.to_owned(),
                    GraphObject {
                        kind: kind.to_owned(),
                        value: decode_strict(bytes)?,
                    },
                ))
            })
            .collect::<Result<_>>()?;
        let proposal = Self {
            format: ResumeFormat::TerminalResume,
            terminal: terminal.clone(),
            archive: archive.generation_root(),
            old_members,
            intent,
            registry: prepared.commitment().clone(),
            source_business_generation: verified.state.business_generation,
            source_topology_generation: verified.state.topology_generation,
            objects,
        };
        proposal.verify()?;
        proposal.verify_source(&verified.state)?;
        PreparedMetadataObject::new(PROPOSAL_KIND, &proposal, MAX_METADATA)?;
        Ok(proposal)
    }

    pub(super) fn hash(&self) -> Result<String> {
        Ok(object_hash(PROPOSAL_KIND, self)?)
    }

    pub(super) fn acceptance_hash(&self, disk_id: &str) -> Result<String> {
        let identity = self.intent.added_roots.get(disk_id).ok_or_else(|| {
            Error::new(
                ErrorCode::DiskIdentityMismatch,
                "resume acceptance is outside the exact target set",
            )
        })?;
        Ok(object_hash(
            ACCEPTANCE_KIND,
            &Acceptance {
                proposal_hash: self.hash()?,
                terminal_authority_hash: self.terminal.hash()?,
                archive_root_hash: self.terminal.archive_root_hash().to_owned(),
                identity: identity.clone(),
            },
        )?)
    }

    pub(super) fn verify(&self) -> Result<VerifiedDiskRegistry> {
        self.terminal.validate_intent()?;
        let (old_intent, old_members) = self
            .old_members
            .verify(&self.archive, &self.terminal.transition_hash()?)?;
        if object_hash(INTENT_KIND, &old_intent)? != self.terminal.transition_hash()?
            || object_hash("membership-control-archive", &self.archive)?
                != self.terminal.archive_root_hash()
            || old_members.len() != self.terminal.original().metadata_members().len()
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "resume omits the exact original fixed membership evidence",
            ));
        }
        self.verify_intent()?;
        let source = self
            .terminal
            .source_registry_at(self.source_topology_generation)?;
        let terminal = source
            .verify_terminal_successor(self.terminal.registry_commitment(), |kind, hash| {
                self.terminal.read_graph_object(kind, hash)
            })?;
        let request = terminal.exit();
        if request.target != self.terminal.target()
            || request.operation_id != self.terminal.operation_id()
            || request.principal != old_intent.principal
            || request.intent_hash != self.terminal.transition_hash()?
            || request.request_digest != request.intent_hash
            || !matches!(
                (&request.effect, self.terminal.action()),
                (
                    crate::RegistryExitEffect::Disable,
                    MembershipExitAction::PlannedOffline
                ) | (
                    crate::RegistryExitEffect::Detach { .. },
                    MembershipExitAction::Deregister
                )
            )
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "resume terminal registry differs from its exact certified exit",
            ));
        }
        let boundary = old_intent
            .boundary
            .as_ref()
            .expect("invariant: verified terminal boundary");
        terminal.verify_resume_at(
            RegistryBoundary {
                authority: &self.terminal.hash()?,
                snapshot: &boundary.snapshot_hash,
                sequence: boundary.sequence,
                topology_generation: self.source_topology_generation,
            },
            &self.intent.target,
            &self.registry,
            |kind, hash| self.read(kind, hash),
        )
    }

    fn verify_intent(&self) -> Result<()> {
        let old = self.terminal.intent();
        let Origin::TerminalResume {
            config,
            predecessor_authority_hash,
            terminal_authority_hash,
        } = &self.intent.origin
        else {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "terminal resumption cannot restart Genesis or use ordinary Add",
            ));
        };
        let terminal_sequence = old.fence_sequence.checked_add(1).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "terminal resume sequence exhausted",
            )
        })?;
        if terminal_authority_hash != &self.terminal.hash()?
            || predecessor_authority_hash.as_deref() != old.origin.predecessor()
            || !pending::activation::authority_config_matches(config, old.origin.config())
            || self.intent.effect.is_some()
            || self.intent.fence_scope.is_some()
            || self.intent.boundary != old.boundary
            || self.intent.fence_sequence != terminal_sequence
            || self.intent.operation_id == old.operation_id
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "resume intent does not extend its exact completed terminal handoff",
            ));
        }
        Engine::require_actor(&self.intent.principal)?;
        validate_target(config, &self.intent.target)?;
        pending::reservation::verify_intent_roots(&self.intent)?;
        Ok(())
    }

    pub(super) fn verify_source(&self, state: &State) -> Result<()> {
        let boundary = self
            .terminal
            .intent()
            .boundary
            .as_ref()
            .expect("invariant: verified terminal boundary");
        if state.sequence != boundary.sequence
            || state.business_generation != self.source_business_generation
            || state.topology_generation != self.source_topology_generation
            || state.operations.contains_key(&self.intent.operation_id)
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "resume source differs from complete frozen history or reuses an operation",
            ));
        }
        self.terminal.management_state(state)?;
        Ok(())
    }

    fn read(&self, kind: &'static str, hash: &str) -> Result<Vec<u8>> {
        let value = self.objects.get(hash).ok_or_else(|| {
            Error::new(
                ErrorCode::ObjectCorrupt,
                "resume registry graph object missing",
            )
        })?;
        if value.kind != kind {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "resume registry graph kind differs",
            ));
        }
        let object = PreparedMetadataObject::new(kind, &value.value, MAX_METADATA)?;
        if object.hash != hash {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "resume registry graph hash differs",
            ));
        }
        Ok(object.bytes().to_vec())
    }
}

impl ResumeCore {
    pub(super) fn new(
        proposal: ResumeProposal,
        votes: BTreeMap<String, DecisionReceipt>,
    ) -> Result<Self> {
        let core = Self {
            format: ResumeFormat::TerminalResume,
            proposal,
            votes,
        };
        core.intent()?;
        PreparedMetadataObject::new(pending::activation::AUTHORITY_KIND, &core, MAX_METADATA)?;
        Ok(core)
    }

    pub(in crate::engine::membership_transition) fn intent(&self) -> Result<TransitionIntent> {
        self.proposal.verify()?;
        let intent = &self.proposal.intent;
        if self.votes.len() != intent.target.metadata_members().len() {
            return Err(Error::new(
                ErrorCode::MetadataQuorumUnavailable,
                "resume authority lacks every configured target decision",
            ));
        }
        let transition = object_hash(INTENT_KIND, intent)?;
        let proposal = self.proposal.hash()?;
        for disk in intent.target.metadata_members() {
            let identity = &intent.added_roots[&disk.disk_id];
            let vote = self.votes.get(&disk.disk_id).ok_or_else(|| {
                Error::new(
                    ErrorCode::MetadataQuorumUnavailable,
                    "resume target decision missing",
                )
            })?;
            vote.verify_terminal_resume(
                &transition,
                identity,
                &proposal,
                &self.proposal.acceptance_hash(&disk.disk_id)?,
            )?;
        }
        Ok(intent.clone())
    }

    pub(in crate::engine::membership_transition) fn registry(&self) -> &DiskRegistryCommitment {
        &self.proposal.registry
    }
    pub(in crate::engine::membership_transition) fn verified_registry(
        &self,
    ) -> Result<VerifiedDiskRegistry> {
        self.proposal.verify()
    }
    pub(in crate::engine::membership_transition) fn terminal(&self) -> &TerminalAuthority {
        &self.proposal.terminal
    }
    pub(in crate::engine::membership_transition) fn archive(&self) -> &ArchiveRoot {
        &self.proposal.archive
    }
    pub(in crate::engine::membership_transition) fn archive_intent(&self) -> &TransitionIntent {
        self.proposal.terminal.intent()
    }
    pub(in crate::engine::membership_transition) fn verify_source(
        &self,
        source: &State,
    ) -> Result<()> {
        self.proposal.verify_source(source)
    }
    pub(in crate::engine::membership_transition) fn management_state(
        &self,
        source: &State,
    ) -> Result<Arc<State>> {
        self.verify_source(source)?;
        Ok(self.proposal.terminal.management_state(source)?.0)
    }

    pub(in crate::engine::membership_transition) fn verify_member(
        &self,
        root: &DiskRoot,
        disk: &DiskConfig,
    ) -> Result<()> {
        let intent = self.intent()?;
        let identity = intent.added_roots.get(&disk.disk_id).ok_or_else(|| {
            Error::new(
                ErrorCode::DiskIdentityMismatch,
                "resume member is outside its fixed target set",
            )
        })?;
        identity.verify_physical_root(root)?;
        let transition = object_hash(INTENT_KIND, &intent)?;
        let store = decision::DecisionStore::open(root, identity, &transition, false)?;
        let observed = store.terminal_resume_receipt(
            &self.proposal.hash()?,
            &self.proposal.acceptance_hash(&disk.disk_id)?,
            false,
        )?;
        if self.votes.get(&disk.disk_id) != Some(&observed) {
            return Err(Error::new(
                ErrorCode::LogFork,
                "resume certificate differs from this member's own durable decision",
            ));
        }
        let parent = root.private_child(&pending::member_directory(disk, &transition))?;
        let saved: ResumeCore = decode_strict(
            &parent.read_bytes(&parent.path().join("resume-certificate.json"), MAX_METADATA)?,
        )?;
        if object_hash(pending::activation::AUTHORITY_KIND, &saved)?
            != object_hash(pending::activation::AUTHORITY_KIND, self)?
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "member has another terminal resume certificate",
            ));
        }
        let old_transition = self.terminal().transition_hash()?;
        let archive = PendingControlArchive::from_installed(
            parent.private_child(&parent.path().join(pending::CONTROL_ARCHIVE_DIRECTORY))?,
            self.terminal().archive_root_hash(),
            &old_transition,
        )?;
        let (state, _) = archive.generation_base(self.archive_intent())?;
        self.verify_source(&state)
    }
}

pub(in crate::engine::membership_transition) fn validate_target(
    original: &ValidatedConfig,
    target: &ValidatedConfig,
) -> Result<()> {
    if !admission::same_settings(original, target) || target.metadata_members().len() == 0 {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "resume must preserve pool settings and introduce a nonempty metadata group",
        ));
    }
    let old_slots = original
        .disks
        .iter()
        .map(|disk| disk.slot_id.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    for old in &original.disks {
        let mut expected = old.clone();
        expected.metadata_member = false;
        if !target
            .disks
            .iter()
            .any(|new| admission::same_disk(&expected, new))
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "resume must preserve all historical bindings without reviving old voters",
            ));
        }
    }
    for new in target
        .disks
        .iter()
        .filter(|disk| !original.disks.iter().any(|old| old.disk_id == disk.disk_id))
    {
        if !new.metadata_member
            || new.binding_generation != 1
            || old_slots.contains(&new.slot_id.to_ascii_lowercase())
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "resume accepts only fresh generation-one metadata directories",
            ));
        }
    }
    Ok(())
}

impl ResumeCore {
    pub(in crate::engine::membership_transition) fn activation_mask_hash(
        &self,
    ) -> Result<Option<String>> {
        let source = self
            .terminal()
            .source_registry_at(self.proposal.source_topology_generation)?;
        let terminal = source
            .verify_terminal_successor(self.terminal().registry_commitment(), |kind, hash| {
                self.terminal().read_graph_object(kind, hash)
            })?;
        Ok(match &terminal.exit().effect {
            crate::RegistryExitEffect::Disable => None,
            crate::RegistryExitEffect::Detach {
                occurrence_mask_hash,
            } => Some(occurrence_mask_hash.clone()),
        })
    }

    pub(in crate::engine::membership_transition) fn current_namespaces(
        &self,
        state: &State,
        previous: Option<&crate::engine::disk_registry::current::CurrentNamespaces>,
    ) -> Result<crate::engine::disk_registry::current::PreparedCurrentNamespaces> {
        use crate::engine::disk_registry::published::occurrence::FrozenDetachSource;
        self.verify_source(state)?;
        let resumed = self.verified_registry()?;
        let authority = self.terminal();
        let predecessor = authority.source_registry_at(state.topology_generation)?;
        let terminal = predecessor
            .verify_terminal_successor(authority.registry_commitment(), |kind, hash| {
                authority.read_graph_object(kind, hash)
            })?;
        let boundary = authority
            .intent()
            .boundary
            .as_ref()
            .expect("invariant: verified terminal boundary");
        let target = &terminal.exit().target;
        let had_registry = authority
            .predecessor()
            .map(|parent| parent.verified_registry())
            .transpose()?
            .flatten()
            .is_some();
        if had_registry != previous.is_some() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "terminal resume lacks its exact replayed predecessor namespace",
            ));
        }
        let source = match previous {
            Some(previous) => FrozenDetachSource::capture_current(
                state,
                authority.original(),
                &boundary.snapshot_hash,
                authority.intent().origin.predecessor().ok_or_else(|| {
                    Error::new(
                        ErrorCode::PublicationInvalid,
                        "terminal namespace has no exact predecessor",
                    )
                })?,
                previous,
                &predecessor,
                target,
            ),
            None => FrozenDetachSource::capture_state(
                state,
                authority.original(),
                Some(&boundary.snapshot_hash),
                authority.intent().origin.predecessor(),
                &predecessor,
                target,
            ),
        }
        .map_err(resume_projection_error)?;
        let prepared = match &terminal.exit().effect {
            crate::RegistryExitEffect::Disable => {
                source.prepare_disabled_namespaces(terminal.verified_registry())
            }
            crate::RegistryExitEffect::Detach {
                occurrence_mask_hash,
            } => source
                .verify_mask(occurrence_mask_hash, |kind, hash| {
                    authority.read_graph_object(kind, hash)
                })
                .and_then(|mask| mask.prepare_current_namespaces(terminal.verified_registry())),
        }
        .map_err(resume_projection_error)?;
        prepared.continue_terminal_registry(&terminal, &resumed)
    }
}

fn resume_projection_error(
    error: crate::engine::disk_registry::published::occurrence::ProjectionError,
) -> Error {
    Error::from(std::io::Error::other(error)).context(
        ErrorCode::PublicationInvalid,
        "terminal resume must preserve the exact inherited namespace and detach mask",
    )
}

impl ResumeCore {
    /// Typed old authority plus the complete independently checked new decision
    /// certificate selects these historical observations. They only expand the
    /// restrictive locator; actual roots must still pass admission afterwards.
    pub(in crate::engine) fn authenticated_roots(
        &self,
        terminal: &VerifiedTerminalAuthority,
    ) -> Result<Vec<(DiskConfig, platform_io::fs_safety::linux::FileIdentity)>> {
        let intent = self.intent()?;
        if self.terminal().hash()? != terminal.authority.hash()? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "resume locators belong to another terminal authority",
            ));
        }
        self.verify_source(&terminal.state)?;
        Ok(intent
            .target
            .metadata_members()
            .map(|disk| {
                (
                    disk.clone(),
                    intent.added_roots[&disk.disk_id].physical_identity(),
                )
            })
            .collect())
    }
}
