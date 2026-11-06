//! Acyclic portable authority: source archive and registry precede the proposal;
//! independently synchronized member decisions bind that exact proposal.
use super::super::archive::persistent::abort_view::ArchivedAbortMember;
use super::*;

const PROPOSAL_KIND: &str = "disk-readmission-proposal";
const ACCEPTANCE_KIND: &str = "disk-readmission-acceptance";

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "retained-directory-readmission")]
    Readmission,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct GraphObject {
    kind: String,
    value: json_codec::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Proposal {
    transition_hash: String,
    archive: ArchiveRoot,
    archive_projection: PortableAbortArchive,
    parent: DiskRegistryCommitment,
    registry: DiskRegistryCommitment,
    #[serialization(with = "storage_contract::u64_string")]
    business_generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    topology_generation: u64,
    objects: BTreeMap<String, GraphObject>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct ReadmissionCore {
    format: Format,
    proposal: Proposal,
    decisions: BTreeMap<String, DecisionReceipt>,
    target_decision: DecisionReceipt,
}

/// Pure admission of one immutable proposal, scoped to this verification call.
/// It does not establish the current identity or durability of any disk root.
struct VerifiedProposal {
    hash: String,
    intent: TransitionIntent,
    members: BTreeMap<String, ArchivedAbortMember>,
    registry: VerifiedDiskRegistry,
}

#[derive(Serialize)]
#[serialization(rename_all = "kebab-case")]
enum AcceptanceRole {
    OriginalVoter,
    ReadmittedDirectory,
}
#[derive(Serialize)]
struct Acceptance<'a> {
    proposal_hash: &'a str,
    identity: &'a MemberIdentity,
    role: AcceptanceRole,
    original_seal_hash: String,
    original_fence: Option<&'a Receipt>,
}

impl Proposal {
    pub(super) fn prepare(engine: &Engine, source: &PendingControlArchive) -> Result<Self> {
        let fence = engine.membership_fence.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "readmission has no admitted fence",
            )
        })?;
        let request = request(&fence.intent)?;
        let previous = engine
            .config
            .generation()
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "readmission has no published predecessor",
                )
            })?
            .portable
            .verified_registry()?
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "readmission has no authenticated registry",
                )
            })?;
        if previous.commitment() != &request.predecessor_registry
            || engine
                .current_namespaces
                .as_ref()
                .is_none_or(|view| view.registry() != previous.commitment())
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "readmission registry source has not been admitted",
            ));
        }
        let boundary = fence
            .intent
            .boundary
            .as_ref()
            .expect("invariant: admitted readmission boundary");
        if engine.state.sequence != boundary.sequence
            || engine.snapshot.as_ref() != Some(&boundary.snapshot_hash)
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "readmission source advanced beyond its fence",
            ));
        }
        let prepared = previous.prepare_readmission(
            fence.intent.origin.config(),
            &fence.intent.target,
            RegistryBoundary {
                authority: fence
                    .intent
                    .origin
                    .predecessor()
                    .expect("invariant: readmission has parent"),
                snapshot: &boundary.snapshot_hash,
                sequence: boundary.sequence,
                topology_generation: engine.state.topology_generation,
            },
            &request.target,
        )?;
        let mut objects = BTreeMap::new();
        add_objects(&mut objects, previous.prepared_copy()?.objects())?;
        add_objects(&mut objects, prepared.objects())?;
        let (_, archive_projection) = source.abort_material()?;
        let proposal = Self {
            transition_hash: fence.hash.clone(),
            archive: source.generation_root(),
            archive_projection,
            parent: previous.commitment().clone(),
            registry: prepared.commitment().clone(),
            business_generation: engine.state.business_generation,
            topology_generation: engine.state.topology_generation,
            objects,
        };
        proposal.verify()?;
        Ok(proposal)
    }
    pub(super) fn hash(&self) -> Result<String> {
        Ok(object_hash(PROPOSAL_KIND, self)?)
    }
    fn read(&self, kind: &'static str, hash: &str) -> Result<Vec<u8>> {
        let object = self.objects.get(hash).ok_or_else(|| {
            Error::new(
                ErrorCode::ObjectCorrupt,
                "readmission graph object is missing",
            )
        })?;
        if object.kind != kind {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "readmission graph object kind differs",
            ));
        }
        let prepared = PreparedMetadataObject::new(kind, &object.value, MAX_METADATA)?;
        if prepared.hash != hash {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "readmission graph object hash differs",
            ));
        }
        Ok(prepared.bytes().to_vec())
    }
    fn verify(&self) -> Result<VerifiedProposal> {
        let (intent, members) = self
            .archive_projection
            .verify(&self.archive, &self.transition_hash)?;
        let request = request(&intent)?;
        request.verify_target(&intent)?;
        if object_hash(INTENT_KIND, &intent)? != self.transition_hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "readmission archive names another transition",
            ));
        }
        if self.parent != request.predecessor_registry {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "readmission parent differs",
            ));
        }
        let previous =
            VerifiedDiskRegistry::read_graph(&request.predecessor_registry, |kind, hash| {
                self.read(kind, hash)
            })?;
        let boundary = intent
            .boundary
            .as_ref()
            .expect("invariant: readmission boundary verified");
        let registry = previous.verify_readmission(
            intent.origin.config(),
            &intent.target,
            RegistryBoundary {
                authority: intent
                    .origin
                    .predecessor()
                    .expect("invariant: readmission parent verified"),
                snapshot: &boundary.snapshot_hash,
                sequence: boundary.sequence,
                topology_generation: self.topology_generation,
            },
            &request.target,
            &self.registry,
            |kind, hash| self.read(kind, hash),
        )?;
        // Independent members mean distinct directory identities, not devices or
        // filesystems. Several registered directories may share one filesystem.
        let mut identities = BTreeSet::new();
        identities.insert((request.directory.device, request.directory.inode));
        for member in members.values() {
            if !identities.insert((member.seal.device, member.seal.inode)) {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "readmission witnesses alias a directory",
                ));
            }
        }
        Ok(VerifiedProposal {
            hash: self.hash()?,
            intent,
            members,
            registry,
        })
    }
    pub(super) fn choose(&self, root: &DiskRoot, disk: &DiskConfig) -> Result<DecisionReceipt> {
        let verified = self.verify()?;
        let (identity, acceptance) = verified.acceptance(&disk.disk_id)?;
        let store = DecisionStore::open(root, &identity, &self.transition_hash, true)?;
        store.readmission_receipt(&verified.hash, &acceptance)
    }
}

impl VerifiedProposal {
    fn acceptance(&self, disk_id: &str) -> Result<(MemberIdentity, String)> {
        let request = request(&self.intent)?;
        if disk_id == request.target.disk_id {
            let identity = request.directory.clone();
            let acceptance = Acceptance {
                proposal_hash: &self.hash,
                identity: &identity,
                role: AcceptanceRole::ReadmittedDirectory,
                original_seal_hash: object_hash("disk-seal", &request.original_seal)?,
                original_fence: None,
            };
            return Ok((identity.clone(), object_hash(ACCEPTANCE_KIND, &acceptance)?));
        }
        let member = self.members.get(disk_id).ok_or_else(|| {
            Error::new(
                ErrorCode::ConfigConflict,
                "readmission witness is not an original voter",
            )
        })?;
        let disk = self
            .intent
            .origin
            .config()
            .metadata_members()
            .find(|disk| disk.disk_id == disk_id)
            .expect("invariant: archive verified exact original member set");
        let identity = pending::admission::member_identity(
            self.intent.origin.config(),
            disk,
            FileIdentity {
                device: member.seal.device,
                inode: member.seal.inode,
            },
        );
        let acceptance = Acceptance {
            proposal_hash: &self.hash,
            identity: &identity,
            role: AcceptanceRole::OriginalVoter,
            original_seal_hash: object_hash("disk-seal", &member.seal)?,
            original_fence: Some(&member.fence),
        };
        Ok((identity.clone(), object_hash(ACCEPTANCE_KIND, &acceptance)?))
    }
}

impl ReadmissionCore {
    pub(in crate::engine::membership_transition) fn target_predecessor_seal(
        &self,
        disk_id: &str,
    ) -> Result<Option<DiskSeal>> {
        let intent = self.intent()?;
        let request = request(&intent)?;
        Ok((request.target.disk_id == disk_id).then(|| request.original_seal.clone()))
    }
    pub(in crate::engine::membership_transition) fn permits_prior_marker(
        &self,
        disk_id: &str,
        hash: &str,
    ) -> Result<bool> {
        Ok(self.target_predecessor_seal(disk_id)?.is_some_and(|seal| matches!(seal.format, DiskSealFormat::MembershipGeneration { authority_hash } if authority_hash == hash)))
    }

    pub(super) fn finish(
        proposal: Proposal,
        decisions: BTreeMap<String, DecisionReceipt>,
        target_decision: DecisionReceipt,
    ) -> Result<Self> {
        let core = Self {
            format: Format::Readmission,
            proposal,
            decisions,
            target_decision,
        };
        core.intent()?;
        Ok(core)
    }
    pub(in crate::engine::membership_transition) fn archive(&self) -> &ArchiveRoot {
        &self.proposal.archive
    }
    pub(in crate::engine::membership_transition) fn registry(&self) -> &DiskRegistryCommitment {
        &self.proposal.registry
    }
    pub(in crate::engine::membership_transition) fn source_topology_generation(&self) -> u64 {
        self.proposal.topology_generation
    }
    pub(in crate::engine::membership_transition) fn registry_parent(
        &self,
    ) -> &DiskRegistryCommitment {
        &self.proposal.parent
    }
    pub(in crate::engine::membership_transition) fn intent(&self) -> Result<TransitionIntent> {
        Ok(self.verify()?.intent)
    }
    fn verify(&self) -> Result<VerifiedProposal> {
        let verified = self.proposal.verify()?;
        let intent = &verified.intent;
        // Online writes require all configured roots. Carry all original decisions
        // rather than shrinking the fixed denominator to the witnesses on hand.
        if self.decisions.len() != verified.members.len()
            || self.decisions.len() < intent.origin.config().metadata_quorum()
        {
            return Err(Error::new(
                ErrorCode::MetadataQuorumUnavailable,
                "readmission lacks the unchanged original member decisions",
            ));
        }
        for disk in intent.origin.config().metadata_members() {
            let decision = self.decisions.get(&disk.disk_id).ok_or_else(|| {
                Error::new(
                    ErrorCode::MetadataQuorumUnavailable,
                    "readmission original member decision is missing",
                )
            })?;
            let (identity, acceptance) = verified.acceptance(&disk.disk_id)?;
            decision.verify_readmission(
                &self.proposal.transition_hash,
                &identity,
                &verified.hash,
                &acceptance,
            )?;
        }
        let request = request(intent)?;
        let (identity, acceptance) = verified.acceptance(&request.target.disk_id)?;
        self.target_decision.verify_readmission(
            &self.proposal.transition_hash,
            &identity,
            &verified.hash,
            &acceptance,
        )?;
        Ok(verified)
    }
    pub(in crate::engine::membership_transition) fn verified_registry(
        &self,
    ) -> Result<VerifiedDiskRegistry> {
        Ok(self.verify()?.registry)
    }
    pub(in crate::engine::membership_transition) fn verify_source(
        &self,
        state: &State,
    ) -> Result<()> {
        let intent = self.intent()?;
        if state.sequence
            != intent
                .boundary
                .as_ref()
                .expect("invariant: readmission boundary")
                .sequence
            || state.business_generation != self.proposal.business_generation
            || state.topology_generation != self.proposal.topology_generation
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "readmission source State differs",
            ));
        }
        Ok(())
    }
    pub(in crate::engine::membership_transition) fn verify_member(
        &self,
        root: &DiskRoot,
        disk: &DiskConfig,
        generation: &str,
        synchronize: bool,
    ) -> Result<()> {
        let intent = self.intent()?;
        let request = request(&intent)?;
        let expected = if disk.disk_id == request.target.disk_id {
            request.directory.verify_physical_root(root)?;
            let stored =
                decode_seal(&root.read_bytes(&root.path().join("disk.json"), MAX_METADATA)?)?;
            let mut expected = request.original_seal.clone();
            if stored.format != expected.format {
                expected.format = DiskSealFormat::MembershipGeneration {
                    authority_hash: generation.into(),
                };
            }
            validate_stored_seal(&stored, &expected)?;
            &self.target_decision
        } else {
            self.proposal.archive_projection.verify_live_member(
                &self.proposal.archive,
                &self.proposal.transition_hash,
                root,
                disk,
                generation,
            )?;
            self.decisions.get(&disk.disk_id).ok_or_else(|| {
                Error::new(
                    ErrorCode::ConfigConflict,
                    "readmission member is not a voter",
                )
            })?
        };
        let (identity, _) = self.proposal.verify()?.acceptance(&disk.disk_id)?;
        DecisionStore::open(root, &identity, &self.proposal.transition_hash, false)?
            .verify_receipt(expected, synchronize)
    }
}

fn add_objects<'a>(
    objects: &mut BTreeMap<String, GraphObject>,
    values: impl Iterator<Item = (&'static str, &'a str, &'a [u8])>,
) -> Result<()> {
    for (kind, hash, bytes) in values {
        let value: json_codec::Value = decode_strict(bytes)?;
        if let Some(previous) = objects.insert(
            hash.into(),
            GraphObject {
                kind: kind.into(),
                value: value.clone(),
            },
        ) && (previous.kind != kind || previous.value != value)
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "readmission graph collision",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/readmission_core_tests.rs"]
mod tests;
