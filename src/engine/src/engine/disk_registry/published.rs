//! Content-addressed registry preparation and verification, never serving authority.
//! The trust seed is an admitted Engine; successors are checked against that exact
//! predecessor. Publication verification is separate from lifecycle completion.
use super::{DiskRegistrationIdentity, Engine};
use crate::{
    Error, ErrorCode, OperationId, Result, decode_strict,
    disk::MAX_METADATA,
    journal::{
        AuthenticatedMetadataObject, Candidate, PreparedMetadataObject, Publication,
        PublicationSeal, verify_publication_seal,
    },
    metadata_map::{Encoder, ProofMeasure, Reference},
    object_hash,
};
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[allow(
    dead_code,
    reason = "occurrence proofs await atomic lifecycle publication and consumer wiring"
)]
pub(in crate::engine) mod occurrence;

const REGISTRY_KIND: &str = "disk-lifecycle-registry";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskRegistryCommitment {
    root_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    view_generation: u64,
}
impl DiskRegistryCommitment {
    pub fn root_hash(&self) -> &str {
        &self.root_hash
    }
    pub fn view_generation(&self) -> u64 {
        self.view_generation
    }
    pub(crate) fn validate(&self) -> Result<()> {
        digest(&self.root_hash)?;
        if self.view_generation == 0 {
            return Err(invalid("registry generation must be positive"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "kebab-case")]
pub enum RegistrationMode {
    Enabled,
    Disabled,
}

/// Historical topology has no current root path or I/O capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoricalDiskDefinition {
    pub identity: DiskRegistrationIdentity,
    pub level: u32,
    pub upstream_slot_id: Option<String>,
    pub original_metadata_member: bool,
    #[serialization(with = "storage_contract::optional_u64_string")]
    pub capacity_bytes: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Registration {
    identity: DiskRegistrationIdentity,
    mode: RegistrationMode,
}

/// All references precede the proposed registry in the hash dependency graph.
/// Detach binds an occurrence mask, not a target-side recovery backup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RegistryExitEffect {
    Disable,
    Detach { occurrence_mask_hash: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct RegistryExitRequest {
    pub target: DiskRegistrationIdentity,
    pub operation_id: OperationId,
    pub principal: String,
    pub request_digest: String,
    pub intent_hash: String,
    pub effect: RegistryExitEffect,
}
impl RegistryExitRequest {
    fn validate(&self) -> Result<()> {
        Engine::require_actor(&self.principal)?;
        digest(&self.request_digest)?;
        digest(&self.intent_hash)?;
        if let RegistryExitEffect::Detach {
            occurrence_mask_hash,
        } = &self.effect
        {
            digest(occurrence_mask_hash)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct DetachedBinding {
    predecessor: DiskRegistryCommitment,
    request: RegistryExitRequest,
    #[serialization(with = "storage_contract::u64_string")]
    view_generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct RegistrySource {
    pool_id: String,
    membership_authority: Option<String>,
    snapshot_hash: Option<String>,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    #[serialization(with = "storage_contract::u64_string")]
    topology_generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum RegistryFormat {
    #[serialization(rename = "authenticated-disk-registry")]
    Authenticated,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct RegistryRoot {
    format: RegistryFormat,
    source: RegistrySource,
    #[serialization(with = "storage_contract::u64_string")]
    view_generation: u64,
    predecessor: Option<DiskRegistryCommitment>,
    historical_topology: Reference,
    registrations: Reference,
    voters: Reference,
    detached: Reference,
    pending_exit: Option<RegistryExitRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RegistryState {
    source: RegistrySource,
    view_generation: u64,
    predecessor: Option<DiskRegistryCommitment>,
    topology: BTreeMap<String, HistoricalDiskDefinition>,
    registrations: BTreeMap<String, Registration>,
    voters: BTreeMap<String, DiskRegistrationIdentity>,
    detached: BTreeMap<String, DetachedBinding>,
    pending_exit: Option<RegistryExitRequest>,
}

/// Authenticated registry graph only. It does not admit disk resources, choose a
/// current head, prove lifecycle completion, or authorize any service or mutation.
#[derive(Clone, Debug)]
pub struct VerifiedDiskRegistry {
    commitment: DiskRegistryCommitment,
    state: RegistryState,
}
pub struct PreparedDiskRegistry {
    expected: VerifiedDiskRegistry,
    objects: Vec<PreparedMetadataObject>,
}
impl PreparedDiskRegistry {
    pub fn commitment(&self) -> &DiskRegistryCommitment {
        &self.expected.commitment
    }
    pub fn objects(&self) -> impl ExactSizeIterator<Item = (&'static str, &str, &[u8])> {
        self.objects
            .iter()
            .map(|object| (object.kind(), object.hash.as_str(), object.bytes()))
    }
    /// Verify only supplied bytes. Missing/corrupt pages never use the Engine,
    /// another member, or the prepared in-memory graph as a data fallback.
    /// The reader must enforce the existing single-object metadata read limit.
    pub fn verify_with(
        &self,
        read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
    ) -> Result<VerifiedDiskRegistry> {
        let loaded = load_registry(&self.expected.commitment, read)?;
        if loaded != self.expected.state {
            return Err(invalid("registry graph differs from exact preparation"));
        }
        Ok(VerifiedDiskRegistry {
            commitment: self.expected.commitment.clone(),
            state: loaded,
        })
    }
}

impl Engine {
    /// Pure preparation from the admitted authority and published metadata. No
    /// filesystem reads or writes and no replacement of the live registry.
    pub fn prepare_disk_registry_base(&self) -> Result<PreparedDiskRegistry> {
        encode_registry(self.disk_registry_base_state())
    }
    fn disk_registry_base_state(&self) -> RegistryState {
        let mut topology = BTreeMap::new();
        let mut registrations = BTreeMap::new();
        let mut voters = BTreeMap::new();
        for disk in &self.config.disks {
            let identity = DiskRegistrationIdentity {
                pool_id: self.config.pool_id.clone(),
                disk_id: disk.disk_id.clone(),
                slot_id: disk.slot_id.clone(),
                binding_generation: disk.binding_generation,
            };
            topology.insert(
                disk.disk_id.clone(),
                HistoricalDiskDefinition {
                    identity: identity.clone(),
                    level: disk.level,
                    upstream_slot_id: disk.upstream_slot_id.clone(),
                    original_metadata_member: disk.metadata_member,
                    capacity_bytes: disk.capacity_bytes,
                },
            );
            registrations.insert(
                disk.disk_id.clone(),
                Registration {
                    identity: identity.clone(),
                    mode: RegistrationMode::Enabled,
                },
            );
            if disk.metadata_member {
                voters.insert(disk.disk_id.clone(), identity);
            }
        }
        RegistryState {
            source: RegistrySource {
                pool_id: self.config.pool_id.clone(),
                membership_authority: self.config.generation_hash().map(str::to_owned),
                snapshot_hash: self.snapshot.clone(),
                sequence: self.state.sequence,
                topology_generation: self.state.topology_generation,
            },
            view_generation: 1,
            predecessor: None,
            topology,
            registrations,
            voters,
            detached: BTreeMap::new(),
            pending_exit: None,
        }
    }
}

impl VerifiedDiskRegistry {
    pub fn commitment(&self) -> &DiskRegistryCommitment {
        &self.commitment
    }
    pub fn historical_topology(&self) -> impl ExactSizeIterator<Item = &HistoricalDiskDefinition> {
        self.state.topology.values()
    }
    pub fn registration(
        &self,
        identity: &DiskRegistrationIdentity,
    ) -> Result<Option<RegistrationMode>> {
        self.require_identity(identity)?;
        Ok(self
            .state
            .registrations
            .get(&identity.disk_id)
            .map(|entry| entry.mode))
    }
    pub fn is_voter(&self, identity: &DiskRegistrationIdentity) -> Result<bool> {
        self.require_identity(identity)?;
        Ok(self.state.voters.contains_key(&identity.disk_id))
    }
    pub fn voter_count(&self) -> usize {
        self.state.voters.len()
    }
    pub fn pending_exit(&self) -> Option<&RegistryExitRequest> {
        self.state.pending_exit.as_ref()
    }
    fn require_identity(&self, identity: &DiskRegistrationIdentity) -> Result<()> {
        if self
            .state
            .topology
            .get(&identity.disk_id)
            .is_none_or(|disk| disk.identity != *identity)
        {
            return Err(invalid("registry lookup differs from historical binding"));
        }
        Ok(())
    }
    pub fn prepare_exit(&self, request: RegistryExitRequest) -> Result<PreparedDiskRegistry> {
        encode_registry(self.successor_state(request)?)
    }
    fn successor_state(&self, request: RegistryExitRequest) -> Result<RegistryState> {
        if self.state.pending_exit.is_some() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "an unfinished registry exit cannot be crossed by another transition",
            ));
        }
        request.validate()?;
        self.require_identity(&request.target)?;
        let mut next = self.state.clone();
        next.view_generation =
            self.state.view_generation.checked_add(1).ok_or_else(|| {
                Error::new(ErrorCode::ResourceLimit, "registry generation exhausted")
            })?;
        next.predecessor = Some(self.commitment.clone());
        let registration = next
            .registrations
            .get_mut(&request.target.disk_id)
            .ok_or_else(|| invalid("exit target is not currently registered"))?;
        match &request.effect {
            RegistryExitEffect::Disable => {
                if registration.mode != RegistrationMode::Enabled {
                    return Err(invalid("disable requires an enabled registration"));
                }
                registration.mode = RegistrationMode::Disabled;
            }
            RegistryExitEffect::Detach { .. } => {
                next.registrations.remove(&request.target.disk_id);
                next.detached.insert(
                    request.target.disk_id.clone(),
                    DetachedBinding {
                        predecessor: self.commitment.clone(),
                        request: request.clone(),
                        view_generation: next.view_generation,
                    },
                );
            }
        }
        next.voters.remove(&request.target.disk_id);
        if next.voters.is_empty() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "terminal registry authority is not implemented",
            ));
        }
        next.pending_exit = Some(request);
        Ok(next)
    }
    /// Authenticate a proposed successor against this exact predecessor. The
    /// expected root must come from the caller's authenticated transition, not
    /// from the source's own advertised root file.
    /// The reader must enforce the existing single-object metadata read limit.
    pub fn verify_successor(
        &self,
        expected: &DiskRegistryCommitment,
        read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
    ) -> Result<Self> {
        let loaded = load_registry(expected, read)?;
        let request = loaded
            .pending_exit
            .clone()
            .ok_or_else(|| invalid("successor lacks its reserved lifecycle intent"))?;
        if loaded != self.successor_state(request)? {
            return Err(invalid(
                "registry successor differs from prescribed transition",
            ));
        }
        Ok(Self {
            commitment: expected.clone(),
            state: loaded,
        })
    }
    /// Check an exact new-format candidate against a caller-authenticated
    /// authority hash and this authenticated registry. Not active admission.
    pub fn verify_candidate_binding(
        &self,
        bytes: &[u8],
        expected_authority: &str,
    ) -> Result<VerifiedRegistryCandidate> {
        let candidate: Candidate = bounded_decode(bytes)?;
        self.check_candidate(&candidate, expected_authority)?;
        Ok(VerifiedRegistryCandidate {
            candidate_hash: object_hash("candidate", &candidate)?,
            authority_hash: expected_authority.to_owned(),
            registry: self.commitment.clone(),
            sequence: candidate.sequence,
        })
    }
    fn check_candidate(&self, candidate: &Candidate, expected_authority: &str) -> Result<()> {
        digest(expected_authority)?;
        let mut voters = candidate.metadata_disk_ids.clone();
        voters.sort();
        if candidate.registry() != Some(&self.commitment)
            || candidate.generation() != Some(expected_authority)
            || candidate.pool_id != self.state.source.pool_id
            || candidate.sequence <= self.state.source.sequence
            || !voters.iter().eq(self.state.voters.keys())
        {
            return Err(invalid(
                "candidate does not bind the exact registry, generation and voters",
            ));
        }
        Ok(())
    }
    /// Authenticate actual publication evidence without consulting the main
    /// lifecycle operation result. This is registry/publication binding, not a
    /// complete prescribed-D3, lifecycle-completion or serving-authority proof.
    pub fn verify_publication_binding(
        &self,
        candidate_bytes: &[u8],
        publication_bytes: &[u8],
        seal_bytes: &[u8],
        expected_authority: &str,
        expected_candidate: &str,
    ) -> Result<VerifiedRegistryPublication> {
        digest(expected_candidate)?;
        let candidate: Candidate = bounded_decode(candidate_bytes)?;
        self.check_candidate(&candidate, expected_authority)?;
        if object_hash("candidate", &candidate)? != expected_candidate {
            return Err(invalid("registry publication selects another candidate"));
        }
        let publication: Publication = bounded_decode(publication_bytes)?;
        let seal: PublicationSeal = bounded_decode(seal_bytes)?;
        verify_publication_seal(&candidate, &publication, &seal)?;
        // Seal verification proves identical voter IDs and receipt chains. The
        // registry also fixes each voter's slot; a consistent foreign slot is
        // not a receipt from that registered binding.
        for receipt in &seal.receipts {
            let member = self
                .state
                .voters
                .get(&receipt.disk_id)
                .expect("invariant: candidate and seal voter sets match the registry");
            if receipt.slot_id != member.slot_id {
                return Err(invalid(
                    "registry publication receipt has another slot binding",
                ));
            }
        }
        if self
            .state
            .pending_exit
            .as_ref()
            .is_some_and(|pending| pending.operation_id == publication.result.operation_id)
        {
            return Err(invalid(
                "registry publication cannot consume the unfinished main lifecycle operation",
            ));
        }
        Ok(VerifiedRegistryPublication {
            candidate_hash: expected_candidate.to_owned(),
            publication_hash: object_hash("publication", &publication)?,
            registry: self.commitment.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedRegistryCandidate {
    candidate_hash: String,
    authority_hash: String,
    registry: DiskRegistryCommitment,
    sequence: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedRegistryPublication {
    candidate_hash: String,
    publication_hash: String,
    registry: DiskRegistryCommitment,
}

impl VerifiedRegistryCandidate {
    pub fn candidate_hash(&self) -> &str {
        &self.candidate_hash
    }
    pub fn authority_hash(&self) -> &str {
        &self.authority_hash
    }
    pub fn registry(&self) -> &DiskRegistryCommitment {
        &self.registry
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}
impl VerifiedRegistryPublication {
    pub fn candidate_hash(&self) -> &str {
        &self.candidate_hash
    }
    pub fn publication_hash(&self) -> &str {
        &self.publication_hash
    }
    pub fn registry(&self) -> &DiskRegistryCommitment {
        &self.registry
    }
}

fn map<T: Serialize>(encoder: &mut Encoder, values: &BTreeMap<String, T>) -> Result<Reference> {
    let values = values
        .iter()
        .map(|(key, value)| Ok((key.clone(), json_codec::to_value(value)?)))
        .collect::<Result<json_codec::Map<_, _>>>()?;
    Ok(encoder
        .map_root(values, ProofMeasure::Plain)?
        .reference()
        .clone())
}
fn encode_registry(state: RegistryState) -> Result<PreparedDiskRegistry> {
    let mut encoder = Encoder::new(MAX_METADATA);
    let root = RegistryRoot {
        format: RegistryFormat::Authenticated,
        source: state.source.clone(),
        view_generation: state.view_generation,
        predecessor: state.predecessor.clone(),
        historical_topology: map(&mut encoder, &state.topology)?,
        registrations: map(&mut encoder, &state.registrations)?,
        voters: map(&mut encoder, &state.voters)?,
        detached: map(&mut encoder, &state.detached)?,
        pending_exit: state.pending_exit.clone(),
    };
    let commitment = DiskRegistryCommitment {
        root_hash: encoder.object(REGISTRY_KIND, &root)?,
        view_generation: state.view_generation,
    };
    Ok(PreparedDiskRegistry {
        expected: VerifiedDiskRegistry { commitment, state },
        objects: encoder.finish(),
    })
}
fn load_registry(
    expected: &DiskRegistryCommitment,
    mut read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
) -> Result<RegistryState> {
    expected.validate()?;
    let mut load = |kind, hash: &str| {
        AuthenticatedMetadataObject::authenticate(kind, hash, bounded_decode(&read(kind, hash)?)?)
    };
    let root: RegistryRoot =
        json_codec::from_value(load(REGISTRY_KIND, expected.root_hash())?.into_value())?;
    if root.view_generation != expected.view_generation() {
        return Err(invalid("registry root generation differs from commitment"));
    }
    fn read_map<T: for<'de> Deserialize<'de>>(
        reference: &Reference,
        load: &mut impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
    ) -> Result<BTreeMap<String, T>> {
        crate::metadata_map::read_map(reference, load)?
            .into_iter()
            .map(|(key, value)| Ok((key, json_codec::from_value(value)?)))
            .collect()
    }
    Ok(RegistryState {
        source: root.source,
        view_generation: root.view_generation,
        predecessor: root.predecessor,
        topology: read_map(&root.historical_topology, &mut load)?,
        registrations: read_map(&root.registrations, &mut load)?,
        voters: read_map(&root.voters, &mut load)?,
        detached: read_map(&root.detached, &mut load)?,
        pending_exit: root.pending_exit,
    })
}
fn bounded_decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T> {
    if bytes.len() > MAX_METADATA {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "registry metadata exceeds reader limit",
        ));
    }
    Ok(decode_strict(bytes)?)
}
fn digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("invalid registry digest"));
    }
    Ok(())
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::PublicationInvalid, message)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/disk_registry_publication_tests.rs"]
mod tests;
