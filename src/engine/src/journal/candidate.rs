//! A generation-bound envelope for new decisions. Original candidates retain
//! their exact hashed representation; they never acquire a synthetic authority.
use super::*;
use std::ops::{Deref, DerefMut};
use crate::engine::snapshot_initialization::registration::DirectoryInitializationBinding;

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
#[derive(Clone, Debug)]
enum CandidateBinding {
    DirectorySnapshot {
        authority: DirectorySnapshotAuthority,
        material: DirectoryInitializationBinding,
    },
    Genesis,
    Membership(String),
    Registry {
        authority_hash: String,
        commitment: crate::DiskRegistryCommitment,
    },
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct DirectorySnapshotCandidate {
    format: DirectorySnapshotFormat,
    authority: DirectorySnapshotAuthority,
    decision: CandidateData,
    directory_material: DirectoryInitializationBinding,
}
#[derive(Serialize)]
struct DirectorySnapshotCandidateRef<'a> {
    format: DirectorySnapshotFormat,
    authority: &'a DirectorySnapshotAuthority,
    decision: &'a CandidateData,
    directory_material: &'a DirectoryInitializationBinding,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DirectorySnapshotAuthority {
    Genesis {},
    Membership { authority_hash: String },
    Registry { authority_hash: String, registry: crate::DiskRegistryCommitment },
}
impl DirectorySnapshotAuthority {
    fn generation(&self) -> Option<&str> {
        match self {
            Self::Genesis {} => None,
            Self::Membership { authority_hash } | Self::Registry { authority_hash, .. } => {
                Some(authority_hash)
            }
        }
    }
    fn registry(&self) -> Option<&crate::DiskRegistryCommitment> {
        match self {
            Self::Registry { registry, .. } => Some(registry),
            Self::Genesis {} | Self::Membership { .. } => None,
        }
    }
}
#[derive(Serialize, Deserialize)]
enum DirectorySnapshotFormat {
    #[serialization(rename = "directory-snapshot-candidate-v1")]
    DirectorySnapshot,
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
    DirectorySnapshot(DirectorySnapshotCandidate),
    Lifecycle(LifecycleCandidate),
    Generation(GenerationCandidate),
    Original(CandidateData),
}
impl Candidate {
    pub(crate) fn new_directory_snapshot(
        data: CandidateData,
        generation: Option<&str>,
        registry: Option<&crate::DiskRegistryCommitment>,
        material: DirectoryInitializationBinding,
    ) -> crate::Result<Self> {
        let authority = match (generation, registry) {
            (None, None) => DirectorySnapshotAuthority::Genesis {},
            (Some(hash), None) => DirectorySnapshotAuthority::Membership {
                authority_hash: hash.to_owned(),
            },
            (Some(hash), Some(registry)) => DirectorySnapshotAuthority::Registry {
                authority_hash: hash.to_owned(),
                registry: registry.clone(),
            },
            (None, Some(_)) => return Err(crate::Error::new(
                crate::ErrorCode::PublicationInvalid,
                "directory registry requires an admitted generation",
            )),
        };
        validate_directory_snapshot(&data, &authority, &material)?;
        Ok(Self {
            data,
            binding: CandidateBinding::DirectorySnapshot { authority, material },
        })
    }
    pub(crate) fn directory_material(&self) -> Option<&DirectoryInitializationBinding> {
        match &self.binding {
            CandidateBinding::DirectorySnapshot { material, .. } => Some(material),
            _ => None,
        }
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
            CandidateBinding::DirectorySnapshot { authority, .. } => authority.generation(),
        }
    }
    pub(crate) fn registry(&self) -> Option<&crate::DiskRegistryCommitment> {
        match &self.binding {
            CandidateBinding::Registry { commitment, .. } => Some(commitment),
            CandidateBinding::DirectorySnapshot { authority, .. } => authority.registry(),
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
            CandidateBinding::DirectorySnapshot { authority, material } => {
                // DerefMut exists for historical callers: recheck closure at encoding too.
                validate_directory_snapshot(&self.data, authority, material)
                    .map_err(data_serialization::ser::Error::custom)?;
                DirectorySnapshotCandidateRef {
                    format: DirectorySnapshotFormat::DirectorySnapshot,
                    authority,
                    decision: &self.data,
                    directory_material: material,
                }.serialize(serializer)
            }
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
            StoredCandidate::DirectorySnapshot(stored) => Self::new_directory_snapshot(
                stored.decision,
                stored.authority.generation(),
                stored.authority.registry(),
                stored.directory_material,
            ).map_err(data_serialization::de::Error::custom),
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

fn validate_directory_snapshot(
    data: &CandidateData,
    authority: &DirectorySnapshotAuthority,
    material: &DirectoryInitializationBinding,
) -> crate::Result<()> {
    if authority.generation().is_some_and(|hash| !valid_authority_hash(hash)) {
        return Err(crate::Error::new(crate::ErrorCode::PublicationInvalid,
            "invalid directory snapshot authority hash"));
    }
    if let Some(registry) = authority.registry() {
        registry.validate()?;
    }
    material.validate(data.operation_id, &data.pool_id).map_err(|error| {
        crate::Error::new(crate::ErrorCode::PublicationInvalid, error.to_string())
    })?;
    Ok(())
}
