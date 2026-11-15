//! A generation-bound envelope for new decisions. Original candidates retain
//! their exact hashed representation; they never acquire a synthetic authority.
use super::*;
use std::ops::{Deref, DerefMut};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct CandidateData {
    pub pool_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub transaction_id: String,
    pub operation_id: OperationId,
    pub request_digest: String,
    pub previous_snapshot: Option<String>,
    pub snapshot_hash: String,
    pub versions: Vec<VersionEvidence>,
    pub metadata_disk_ids: Vec<String>,
    pub history_roots: crate::metadata_map::Reference,
}

#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    data: CandidateData,
    binding: CandidateBinding,
}

/// An encode-only preflight's exact fields and original envelope binding.
/// This is not serializable, mutable, cloneable or a durable decision proof.
pub(crate) struct CandidatePreparation {
    data: CandidateData,
    binding: CandidateBinding,
}
impl CandidatePreparation {
    pub(crate) fn versions(&self) -> &[VersionEvidence] {
        &self.data.versions
    }

    pub(crate) fn finish(self) -> Candidate {
        Candidate { data: self.data, binding: self.binding }
    }
}
#[derive(Clone, Debug)]
enum CandidateBinding {
    Genesis,
    Membership(String),
    Registry {
        authority_hash: String,
        commitment: crate::DiskRegistryCommitment,
    },
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct LifecycleCandidate {
    format: LifecycleFormat,
    authority_hash: String,
    registry: crate::DiskRegistryCommitment,
    decision: CandidateData,
}
#[derive(Serialize)]
struct LifecycleCandidateRef<'a> {
    format: LifecycleFormat,
    authority_hash: &'a str,
    registry: &'a crate::DiskRegistryCommitment,
    decision: &'a CandidateData,
}
#[derive(Serialize, Deserialize)]
enum LifecycleFormat {
    #[serialization(rename = "disk-lifecycle-candidate")]
    DiskLifecycle,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct GenerationCandidate {
    format: GenerationFormat,
    authority_hash: String,
    decision: CandidateData,
}
#[derive(Serialize)]
struct GenerationCandidateRef<'a> {
    format: GenerationFormat,
    authority_hash: &'a str,
    decision: &'a CandidateData,
}
#[derive(Serialize, Deserialize)]
enum GenerationFormat {
    #[serialization(rename = "membership-generation-candidate")]
    MembershipGeneration,
}
#[derive(Deserialize)]
#[serialization(untagged)]
enum StoredCandidate {
    Lifecycle(LifecycleCandidate),
    Generation(GenerationCandidate),
    Original(CandidateData),
}
impl Candidate {
    /// Keep the exact budgeted envelope across physical preparation. Finishing
    /// moves it back unchanged rather than selecting or reconstructing a binding.
    pub(crate) fn into_preparation(self) -> CandidatePreparation {
        CandidatePreparation { data: self.data, binding: self.binding }
    }

    pub(crate) fn new(data: CandidateData, generation: Option<&str>) -> Self {
        Self {
            data,
            binding: match generation {
                None => CandidateBinding::Genesis,
                Some(hash) => CandidateBinding::Membership(hash.to_owned()),
            },
        }
    }
    pub(crate) fn new_with_registry(
        data: CandidateData,
        authority: &str,
        commitment: &crate::DiskRegistryCommitment,
    ) -> Self {
        Self {
            data,
            binding: CandidateBinding::Registry {
                authority_hash: authority.to_owned(),
                commitment: commitment.clone(),
            },
        }
    }
    pub(crate) fn generation(&self) -> Option<&str> {
        match &self.binding {
            CandidateBinding::Genesis => None,
            CandidateBinding::Membership(hash)
            | CandidateBinding::Registry {
                authority_hash: hash,
                ..
            } => Some(hash),
        }
    }
    pub(crate) fn registry(&self) -> Option<&crate::DiskRegistryCommitment> {
        match &self.binding {
            CandidateBinding::Registry { commitment, .. } => Some(commitment),
            CandidateBinding::Genesis | CandidateBinding::Membership(_) => None,
        }
    }
}
impl Deref for Candidate {
    type Target = CandidateData;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}
impl DerefMut for Candidate {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}
impl Serialize for Candidate {
    fn serialize<S: data_serialization::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match &self.binding {
            CandidateBinding::Genesis => self.data.serialize(serializer),
            CandidateBinding::Membership(authority_hash) => GenerationCandidateRef {
                format: GenerationFormat::MembershipGeneration,
                authority_hash,
                decision: &self.data,
            }
            .serialize(serializer),
            CandidateBinding::Registry {
                authority_hash,
                commitment,
            } => LifecycleCandidateRef {
                format: LifecycleFormat::DiskLifecycle,
                authority_hash,
                registry: commitment,
                decision: &self.data,
            }
            .serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for Candidate {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        match StoredCandidate::deserialize(deserializer)? {
            StoredCandidate::Lifecycle(stored) => {
                if !valid_authority_hash(&stored.authority_hash) {
                    return Err(data_serialization::de::Error::custom(
                        "invalid lifecycle authority hash",
                    ));
                }
                stored
                    .registry
                    .validate()
                    .map_err(data_serialization::de::Error::custom)?;
                Ok(Self {
                    data: stored.decision,
                    binding: CandidateBinding::Registry {
                        authority_hash: stored.authority_hash,
                        commitment: stored.registry,
                    },
                })
            }
            StoredCandidate::Original(data) => Ok(Self::new(data, None)),
            StoredCandidate::Generation(stored) => {
                if !valid_authority_hash(&stored.authority_hash) {
                    return Err(data_serialization::de::Error::custom(
                        "invalid membership generation hash",
                    ));
                }
                Ok(Self::new(stored.decision, Some(&stored.authority_hash)))
            }
        }
    }
}

fn valid_authority_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
