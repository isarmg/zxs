//! Certified cancellation of an uncompleted pure addition. Immutable generation
//! identity is separate from any fixed-original-majority witness.
use super::*;
use crate::metadata_map::Reference;
use crate::metadata_map::{Encoder, ProofMeasure};
use archive::persistent::abort_view::VerifiedAbortArchive;
use archive::persistent::{
    ArchiveRoot,
    abort_view::{ArchivedAbortMember, PortableAbortArchive},
};
use decision::DecisionReceipt;
use decision::DecisionStore;
use pending::{MemberIdentity, activation::AUTHORITY_KIND};
use platform_io::fs_safety::linux::FileIdentity;
use std::collections::BTreeSet;

const ACCEPTANCE_KIND: &str = "membership-abort-acceptance";
const CERTIFICATE_KIND: &str = "membership-abort-certificate";

#[derive(Clone, Debug, Serialize, Deserialize)]
enum CoreFormat {
    #[serialization(rename = "membership-abort-generation")]
    AbortGeneration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct AbortCore {
    format: CoreFormat,
    transition_hash: String,
    pub(super) archive: ArchiveRoot,
}

/// Separate from G. The archive projection binds every original seal and fence
/// to F; the witness proves OLD-Q for G. Neither is a fresh local slot receipt.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct AbortAuthorityEvidence {
    archive: PortableAbortArchive,
    witness: PortableAbortEvidence,
}

impl AbortAuthorityEvidence {
    pub(super) fn status(&self, core: &AbortCore) -> Result<MembershipAbortStatus> {
        let (intent, members) = self.archive.verify(&core.archive, &core.transition_hash)?;
        let context = AbortContext::from_members(core.archive.clone(), &intent, &members)?;
        let verified = self.witness.verify(&context)?;
        Ok(MembershipAbortStatus {
            operation_id: intent.operation_id,
            transition_hash: core.transition_hash.clone(),
            generation_hash: context.generation.hash.clone(),
            confirmed_members: verified.decisions.len(),
            original_members: context.acceptances.len(),
            original_quorum: context.quorum(),
        })
    }

    pub(super) fn verify_member(
        &self,
        core: &AbortCore,
        root: &DiskRoot,
        disk: &DiskConfig,
    ) -> Result<()> {
        let (intent, members) = self.archive.verify(&core.archive, &core.transition_hash)?;
        let context = AbortContext::from_members(core.archive.clone(), &intent, &members)?;
        let verified = self.witness.verify(&context)?;
        self.archive.verify_live_member(
            &core.archive,
            &core.transition_hash,
            root,
            disk,
            &context.generation.hash,
        )?;
        let acceptance = context.acceptance(&disk.disk_id)?;
        let expected = verified.decisions.get(&disk.disk_id);
        let mut path = root.path().join("metadata");
        for part in ["membership-decisions", &core.transition_hash] {
            path.push(part);
            if root.optional_metadata(&path)?.is_none() {
                return if expected.is_some() {
                    Err(Error::new(
                        ErrorCode::RecoveryRequired,
                        "Abort witness lacks its actual original decision",
                    ))
                } else {
                    Ok(())
                };
            }
            root.directory_identity(&path)?;
        }
        let store = DecisionStore::open(root, &acceptance.identity, &core.transition_hash, false)?;
        store.verify_abort_compatible(
            &context.generation.hash,
            &object_hash(ACCEPTANCE_KIND, acceptance)?,
            expected,
        )
    }

    pub(super) fn verify(&self, core: &AbortCore) -> Result<TransitionIntent> {
        let (intent, members) = self.archive.verify(&core.archive, &core.transition_hash)?;
        let context = AbortContext::from_members(core.archive.clone(), &intent, &members)?;
        if context.generation.hash != object_hash(AUTHORITY_KIND, core)? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "abort archive selects another core",
            ));
        }
        self.witness.verify(&context)?;
        Ok(intent)
    }
}

/// A prescribed future result, not a published OperationResult. Its kind, actor,
/// original request and sequence cannot be selected by a cancellation caller.
#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
struct PrescribedAbortOutcome {
    operation_id: OperationId,
    principal: String,
    request_digest: String,
    sequence: u64,
    topology_generation: u64,
}
#[cfg(test)]
impl PrescribedAbortOutcome {
    fn kind(&self) -> &'static str {
        "membership-add-cancelled"
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct AbortAcceptance {
    core_hash: String,
    identity: MemberIdentity,
    original_seal_hash: String,
    original_fence: Receipt,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct AbortVote {
    identity: MemberIdentity,
    acceptance_hash: String,
    decision: DecisionReceipt,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum CertificateFormat {
    #[serialization(rename = "fenced-handoff-abort-certificate")]
    AbortCertificate,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct AbortCertificate {
    format: CertificateFormat,
    core_hash: String,
    votes: Reference,
}

/// Hash-authenticated detached evidence. Decoding this envelope is not a proof
/// of an actual synchronized slot. Online aggregation additionally uses held
/// DecisionStores; portable recovery must supply an independently admitted F.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct PortableAbortEvidence {
    core_hash: String,
    certificate_hash: String,
    objects: BTreeMap<String, json_codec::Value>,
}

/// Private construction prevents caller-supplied config or online counts from
/// changing the denominator, effective membership, core, or prescribed result.
struct AbortContext {
    core: AbortCore,
    generation: PreparedMetadataObject,
    intent: TransitionIntent,
    acceptances: BTreeMap<String, AbortAcceptance>,
}

impl AbortContext {
    fn from_archive(archive: VerifiedAbortArchive) -> Result<Self> {
        Self::from_members(archive.root().clone(), archive.intent(), archive.members())
    }

    fn from_members(
        archive: ArchiveRoot,
        intent: &TransitionIntent,
        members: &BTreeMap<String, ArchivedAbortMember>,
    ) -> Result<Self> {
        Engine::require_actor(&intent.principal)?;
        if intent.fence_sequence == u64::MAX
            || intent.fence_sequence <= intent.boundary.as_ref().map_or(0, |base| base.sequence)
            || (intent.origin.predecessor().is_some() && intent.boundary.is_none())
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "invalid abort frozen boundary",
            ));
        }
        if intent.effect.is_some() {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "abort evidence only admits pure additions",
            ));
        }
        let config = intent.origin.config();
        if !pending::admission::same_settings(config, &intent.target)
            || config.disks.iter().any(|old| {
                !intent
                    .target
                    .disks
                    .iter()
                    .any(|new| pending::admission::same_disk(old, new))
            })
            || intent.target.disks.len() <= config.disks.len()
            || intent.target.disks.iter().any(|disk| {
                !config.disks.iter().any(|old| old.disk_id == disk.disk_id)
                    && disk.binding_generation != 1
            })
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "abort intent is not a pure addition",
            ));
        }
        pending::reservation::verify_intent_roots(intent)?;
        let core = AbortCore {
            format: CoreFormat::AbortGeneration,
            transition_hash: object_hash(INTENT_KIND, intent)?,
            archive,
        };
        let generation = PreparedMetadataObject::new(AUTHORITY_KIND, &core, MAX_METADATA)?;
        let mut acceptances = BTreeMap::new();
        let mut physical = BTreeSet::new();
        for disk in config.metadata_members() {
            let member = &members[&disk.disk_id];
            if !physical.insert((member.seal.device, member.seal.inode)) {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "abort originals repeat a physical root",
                ));
            }
            let identity = pending::admission::member_identity(
                config,
                disk,
                FileIdentity {
                    device: member.seal.device,
                    inode: member.seal.inode,
                },
            );
            acceptances.insert(
                disk.disk_id.clone(),
                AbortAcceptance {
                    core_hash: generation.hash.clone(),
                    identity,
                    original_seal_hash: object_hash("membership-original-seal", &member.seal)?,
                    original_fence: member.fence.clone(),
                },
            );
        }
        Ok(Self {
            core,
            generation,
            intent: intent.clone(),
            acceptances,
        })
    }

    #[cfg(test)]
    fn prescribed_outcome(&self, topology_generation: u64) -> Result<PrescribedAbortOutcome> {
        Ok(PrescribedAbortOutcome {
            operation_id: self.intent.operation_id,
            principal: self.intent.principal.clone(),
            request_digest: self.core.transition_hash.clone(),
            sequence: self
                .intent
                .fence_sequence
                .checked_add(1)
                .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "abort sequence exhausted"))?,
            topology_generation: topology_generation.checked_add(1).ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "abort topology generation exhausted",
                )
            })?,
        })
    }

    fn effective_config(&self) -> &ValidatedConfig {
        self.intent.origin.config()
    }

    fn quorum(&self) -> usize {
        // Fixed O from authenticated I, never target membership or runtime limits.
        self.effective_config().metadata_quorum()
    }

    #[cfg(test)]
    fn verify_core(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_METADATA {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "abort core exceeds metadata limit",
            ));
        }
        let core: AbortCore = decode_strict(bytes)?;
        if object_hash(AUTHORITY_KIND, &core)? != self.generation.hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "abort core differs from the exact frozen intent and archive",
            ));
        }
        Ok(())
    }

    fn acceptance(&self, disk_id: &str) -> Result<&AbortAcceptance> {
        self.acceptances.get(disk_id).ok_or_else(|| {
            Error::new(
                ErrorCode::DiskIdentityMismatch,
                "abort voter is not an original metadata member",
            )
        })
    }

    /// Caller retains the original writer locks through certification. This
    /// method confirms an existing choice only; it cannot install an Abort vote.
    fn observe_vote(
        &self,
        disk_id: &str,
        root: &DiskRoot,
        decision: DecisionReceipt,
    ) -> Result<ObservedAbortVote> {
        let acceptance = self.acceptance(disk_id)?;
        let acceptance_hash = object_hash(ACCEPTANCE_KIND, acceptance)?;
        decision.verify_abort(
            &self.core.transition_hash,
            &acceptance.identity,
            &self.generation.hash,
            &acceptance_hash,
        )?;
        let store = DecisionStore::open(
            root,
            &acceptance.identity,
            &self.core.transition_hash,
            false,
        )?;
        store.verify_receipt(&decision, true)?;
        verify_original_seal(root, acceptance)?;
        Ok(ObservedAbortVote {
            disk_id: disk_id.to_owned(),
            vote: AbortVote {
                identity: acceptance.identity.clone(),
                acceptance_hash,
                decision,
            },
            store,
            root: root.private_child(root.path())?,
        })
    }

    fn certify(&self, observations: &[ObservedAbortVote]) -> Result<PortableAbortEvidence> {
        let mut encoder = Encoder::new(MAX_METADATA);
        let mut votes = json_codec::Map::new();
        for observed in observations {
            let acceptance = self.acceptance(&observed.disk_id)?;
            self.verify_vote(&observed.vote, acceptance)?;
            if votes.contains_key(&observed.disk_id) {
                return Err(Error::new(ErrorCode::LogFork, "duplicate abort voter"));
            }
            // A receipt cached before corruption/replacement is not a new vote.
            observed
                .store
                .verify_receipt(&observed.vote.decision, true)?;
            verify_original_seal(&observed.root, acceptance)?;
            encoder.object(ACCEPTANCE_KIND, acceptance)?;
            votes.insert(
                observed.disk_id.clone(),
                json_codec::to_value(&observed.vote)?,
            );
        }
        if votes.len() < self.quorum() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "abort witness lacks the fixed original majority",
            ));
        }
        let votes = encoder
            .map_root(votes, ProofMeasure::Plain)?
            .reference()
            .clone();
        let certificate_hash = encoder.object(
            CERTIFICATE_KIND,
            &AbortCertificate {
                format: CertificateFormat::AbortCertificate,
                core_hash: self.generation.hash.clone(),
                votes,
            },
        )?;
        let objects = encoder
            .finish()
            .into_iter()
            .map(|object| {
                Ok((
                    format!("{}/{}", object.kind(), object.hash),
                    decode_strict(object.bytes())?,
                ))
            })
            .collect::<Result<_>>()?;
        Ok(PortableAbortEvidence {
            core_hash: self.generation.hash.clone(),
            certificate_hash,
            objects,
        })
    }

    fn verify_vote(&self, vote: &AbortVote, acceptance: &AbortAcceptance) -> Result<()> {
        if vote.identity != acceptance.identity
            || vote.acceptance_hash != object_hash(ACCEPTANCE_KIND, acceptance)?
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "abort acceptance differs from original member and fence",
            ));
        }
        vote.decision.verify_abort(
            &self.core.transition_hash,
            &acceptance.identity,
            &self.generation.hash,
            &vote.acceptance_hash,
        )
    }
}

struct ObservedAbortVote {
    disk_id: String,
    vote: AbortVote,
    store: DecisionStore,
    root: DiskRoot,
}

fn verify_original_seal(root: &DiskRoot, acceptance: &AbortAcceptance) -> Result<()> {
    let seal = decode_seal(&root.read_bytes(&root.path().join("disk.json"), MAX_METADATA)?)?;
    if object_hash("membership-original-seal", &seal)? != acceptance.original_seal_hash {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "abort voter original seal changed",
        ));
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct VerifiedAbortCertificate {
    decisions: BTreeMap<String, DecisionReceipt>,
    #[cfg(test)]
    core_hash: String,
    #[cfg(test)]
    certificate_hash: String,
    #[cfg(test)]
    voters: BTreeSet<String>,
}

impl PortableAbortEvidence {
    /// Portable validation proves the bound evidence closure, not that arbitrary
    /// deserialized records were written to actual slots. Only certify accepts
    /// fresh local durability proof; recovery still needs local slot admission.
    fn verify(&self, context: &AbortContext) -> Result<VerifiedAbortCertificate> {
        if self.core_hash != context.generation.hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "abort evidence selects another core",
            ));
        }
        let mut read = BTreeSet::new();
        let mut load = |kind: &'static str, hash: &str| -> Result<AuthenticatedMetadataObject> {
            let key = format!("{kind}/{hash}");
            let value = self.objects.get(&key).ok_or_else(|| {
                Error::new(
                    ErrorCode::ObjectCorrupt,
                    "abort evidence dependency missing",
                )
            })?;
            read.insert(key);
            AuthenticatedMetadataObject::authenticate(kind, hash, value.clone())
        };
        let certificate: AbortCertificate =
            json_codec::from_value(load(CERTIFICATE_KIND, &self.certificate_hash)?.into_value())?;
        if certificate.core_hash != context.generation.hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "abort certificate selects another core",
            ));
        }
        let votes = crate::metadata_map::read_map(&certificate.votes, &mut load)?;
        if votes.len() < context.quorum() || votes.len() > context.acceptances.len() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "abort certificate lacks a fixed original majority",
            ));
        }
        let mut decisions = BTreeMap::new();
        #[cfg(test)]
        let mut voters = BTreeSet::new();
        for (disk_id, value) in votes {
            let expected = context.acceptance(&disk_id)?;
            let vote: AbortVote = json_codec::from_value(value)?;
            let acceptance: AbortAcceptance =
                json_codec::from_value(load(ACCEPTANCE_KIND, &vote.acceptance_hash)?.into_value())?;
            if &acceptance != expected {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "abort evidence changes an original member or frozen fence",
                ));
            }
            context.verify_vote(&vote, expected)?;
            decisions.insert(disk_id.clone(), vote.decision.clone());
            #[cfg(test)]
            voters.insert(disk_id);
        }
        if read.len() != self.objects.len() {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "abort evidence contains unrelated objects",
            ));
        }
        Ok(VerifiedAbortCertificate {
            decisions,
            #[cfg(test)]
            core_hash: self.core_hash.clone(),
            #[cfg(test)]
            certificate_hash: self.certificate_hash.clone(),
            #[cfg(test)]
            voters,
        })
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/membership_abort_tests.rs"]
pub(in crate::engine::membership_transition) mod tests;

mod runtime;
pub use runtime::MembershipAbortStatus;
