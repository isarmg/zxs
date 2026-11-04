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
    generation: Option<String>,
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
    Generation(GenerationCandidate),
    Original(CandidateData),
}
impl Candidate {
    pub(crate) fn new(data: CandidateData, generation: Option<&str>) -> Self {
        Self {
            data,
            generation: generation.map(str::to_owned),
        }
    }
    pub(crate) fn generation(&self) -> Option<&str> {
        self.generation.as_deref()
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
        match &self.generation {
            None => self.data.serialize(serializer),
            Some(authority_hash) => GenerationCandidateRef {
                format: GenerationFormat::MembershipGeneration,
                authority_hash,
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
            StoredCandidate::Original(data) => Ok(Self::new(data, None)),
            StoredCandidate::Generation(stored) => {
                if stored.authority_hash.len() != 64
                    || !stored
                        .authority_hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(data_serialization::de::Error::custom(
                        "invalid membership generation hash",
                    ));
                }
                Ok(Self::new(stored.decision, Some(&stored.authority_hash)))
            }
        }
    }
}
