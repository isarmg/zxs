//! Display-only observations for one independent source node and a finite list
//! of candidates. Decoding checks structure, not disk evidence or authority.
//! This contract has no HTTP route, producer, complete-discovery claim, or
//! conversion into an engine SourceNode, candidate batch, or write capability.
use data_serialization::{Deserialize, Deserializer, Serialize, de};
use std::{collections::BTreeSet, fmt, marker::PhantomData};

pub const MAX_OBSERVATION_BYTES: usize = 1024 * 1024;
pub const MAX_CANDIDATES: usize = 128;
pub const MAX_NODE_PINS: usize = 4096;
/// A transport budget for repeated pin references, not an own-chain proof.
pub const MAX_DISPLAY_PIN_REFERENCES: usize = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, error_definition::Error)]
pub enum InvalidObservation {
    #[error("independent identifier must be 1..=96 ASCII token bytes")]
    Token,
    #[error("independent digest must be 64 lowercase hexadecimal digits")]
    Digest,
    #[error("binding generation must be nonzero")]
    Generation,
    #[error("checkpoint presence and sequence disagree")]
    Checkpoint,
    #[error("node pins or source/root relation are inconsistent")]
    Node,
    #[error("candidate {index} has an inconsistent observation status")]
    Candidate { index: usize },
    #[error("candidate count exceeds the observation budget")]
    CandidateBudget,
    #[error("display pin references exceed the transport budget")]
    PinBudget,
    #[error("batch counts or incomplete/ambiguous status are inconsistent")]
    Batch,
    #[error("source topology nodes, publications or relations are inconsistent")]
    Topology,
    #[error("source topology exceeds its display budget")]
    TopologyBudget,
    #[error("retained history inventory is inconsistent")]
    History,
    #[error("retained history inventory exceeds its display budget")]
    HistoryBudget,
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

macro_rules! scalar {
    ($name:ident, $check:ident, $error:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
        #[serialization(transparent)]
        pub struct $name(String);
        impl $name {
            pub fn parse(value: &str) -> Result<Self, InvalidObservation> {
                if !$check(value) {
                    return Err(InvalidObservation::$error);
                }
                Ok(Self(value.to_owned()))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::parse(&String::deserialize(d)?).map_err(de::Error::custom)
            }
        }
    };
}
scalar!(IndependentToken, token, Token);
scalar!(PublicationId, digest, Digest);
scalar!(Incarnation, digest, Digest);
scalar!(IndependentCommitId, digest, Digest);
scalar!(IndependentBodyId, digest, Digest);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct BindingGeneration(std::num::NonZeroU64);
impl BindingGeneration {
    pub fn new(value: u64) -> Result<Self, InvalidObservation> {
        std::num::NonZeroU64::new(value)
            .map(Self)
            .ok_or(InvalidObservation::Generation)
    }
    pub fn get(self) -> u64 {
        self.0.get()
    }
}
impl Serialize for BindingGeneration {
    fn serialize<S: data_serialization::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        crate::u64_string::serialize(&self.get(), s)
    }
}
impl<'de> Deserialize<'de> for BindingGeneration {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(crate::u64_string::deserialize(d)?).map_err(de::Error::custom)
    }
}

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
        #[serialization(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                match String::deserialize(d)?.as_str() {
                    $($text => Ok(Self::$variant),)+
                    value => Err(de::Error::unknown_variant(value, &[$($text),+])),
                }
            }
        }
    };
}
string_enum!(ObservationProtocol {
    IndependentSourceObservation => "independent_source_observation",
});
string_enum!(IndependentRole {
    Primary => "primary", Secondary => "secondary", Tertiary => "tertiary",
});
impl IndependentRole {
    pub(crate) fn rank(self) -> u8 {
        match self {
            Self::Primary => 1,
            Self::Secondary => 2,
            Self::Tertiary => 3,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct IndependentOrigin {
    pub pool_id: IndependentToken,
    pub disk_id: IndependentToken,
    pub slot_id: IndependentToken,
    pub binding_generation: BindingGeneration,
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct IndependentIdentity {
    pub origin: IndependentOrigin,
    pub incarnation: Incarnation,
    pub role: IndependentRole,
}

/// Logical checkpoint fields only. Equality is not capture identity or proof
/// that a physical directory is still present.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct IndependentCheckpoint {
    pub identity: IndependentIdentity,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub publication: Option<PublicationId>,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
}
impl IndependentCheckpoint {
    pub(crate) fn validate(&self) -> Result<(), InvalidObservation> {
        if self.publication.is_some() != (self.sequence != 0) {
            return Err(InvalidObservation::Checkpoint);
        }
        Ok(())
    }
}

pub(crate) fn bounded<'de, D: Deserializer<'de>, T: Deserialize<'de>, const N: usize>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    struct List<T, const N: usize>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> de::Visitor<'de> for List<T, N> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "an array of at most {N} items")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = a.next_element()? {
                if values.len() == N {
                    return Err(de::Error::custom("observation array exceeds its budget"));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    d.deserialize_seq(List::<T, N>(PhantomData))
}
pub(crate) fn pins<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<PublicationId>, D::Error> {
    bounded::<D, PublicationId, MAX_NODE_PINS>(d)
}
fn rows<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<CandidateRow>, D::Error> {
    bounded::<D, CandidateRow, MAX_CANDIDATES>(d)
}
pub(crate) fn canonical_pins(pins: &[PublicationId]) -> bool {
    pins.len() <= MAX_NODE_PINS && pins.windows(2).all(|pair| pair[0] < pair[1])
}

/// A display reference to one node, not the engine's opaque SourceNode.
/// The graph producer and request-side capture binding are separate boundaries.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct IndependentNodeReference {
    pub root: IndependentCheckpoint,
    pub identity: IndependentIdentity,
    #[serialization(deserialize_with = "pins")]
    pub pins: Vec<PublicationId>,
}
impl IndependentNodeReference {
    fn validate(&self) -> Result<(), InvalidObservation> {
        self.root.validate()?;
        if !canonical_pins(&self.pins) {
            return Err(InvalidObservation::Node);
        }
        if self.identity == self.root.identity {
            if self.pins.iter().ne(self.root.publication.iter()) {
                return Err(InvalidObservation::Node);
            }
        } else if self.root.publication.is_none()
            || self.pins.is_empty()
            || self.identity.origin == self.root.identity.origin
            || self.identity.role.rank() >= self.root.identity.role.rank()
            || (self.identity.role.rank() + 1 == self.root.identity.role.rank()
                && self.identity.origin.disk_id == self.root.identity.origin.disk_id)
        {
            return Err(InvalidObservation::Node);
        }
        Ok(())
    }
}

// Tagged unit variants otherwise discard surplus fields. Require an empty map
// so a unit status cannot silently carry online, selected or writable claims.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CandidateStatus {
    #[serialization(deserialize_with = "crate::encoding::empty_variant")]
    IdentityMismatch,
    #[serialization(deserialize_with = "crate::encoding::empty_variant")]
    UnpinnedNode,
    MissingPinnedCheckpoints {
        #[serialization(deserialize_with = "pins")]
        missing: Vec<PublicationId>,
    },
    #[serialization(deserialize_with = "crate::encoding::empty_variant")]
    SatisfiesPinnedCheckpoints,
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct CandidateRow {
    pub checkpoint: IndependentCheckpoint,
    pub status: CandidateStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BatchStatus {
    #[serialization(deserialize_with = "crate::encoding::empty_variant")]
    NotObserved,
    IncompleteObservation {
        matching_candidates: usize,
    },
    Ambiguous {
        matching_candidates: usize,
    },
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct WireObservation {
    protocol: ObservationProtocol,
    node: IndependentNodeReference,
    #[serialization(deserialize_with = "rows")]
    observations: Vec<CandidateRow>,
    physical_candidates: usize,
    status: BatchStatus,
}

/// Structurally admitted display data only. A forged-but-consistent payload is
/// not verified disk evidence. HTTP authentication, exact request ownership and
/// a real server-side producer are required before showing any result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(try_from = "WireObservation")]
pub struct IndependentSourceObservation {
    protocol: ObservationProtocol,
    node: IndependentNodeReference,
    observations: Vec<CandidateRow>,
    physical_candidates: usize,
    status: BatchStatus,
}
impl IndependentSourceObservation {
    pub fn new(
        node: IndependentNodeReference,
        observations: Vec<CandidateRow>,
        physical_candidates: usize,
        status: BatchStatus,
    ) -> Result<Self, InvalidObservation> {
        WireObservation {
            protocol: ObservationProtocol::IndependentSourceObservation,
            node,
            observations,
            physical_candidates,
            status,
        }
        .try_into()
    }
    pub fn node(&self) -> &IndependentNodeReference {
        &self.node
    }
    /// Input order includes aliases and rejections. It does not choose a disk.
    pub fn observations(&self) -> &[CandidateRow] {
        &self.observations
    }
    pub fn physical_candidates(&self) -> usize {
        self.physical_candidates
    }
    pub fn status(&self) -> BatchStatus {
        self.status
    }
}
impl TryFrom<WireObservation> for IndependentSourceObservation {
    type Error = InvalidObservation;
    fn try_from(wire: WireObservation) -> Result<Self, Self::Error> {
        if wire.observations.len() > MAX_CANDIDATES {
            return Err(InvalidObservation::CandidateBudget);
        }
        wire.node.validate()?;
        let mut references = wire.node.pins.len();
        let mut matching_rows = 0;
        let mut matching_facts = BTreeSet::new();
        let mut nonmatching_facts = BTreeSet::new();
        for (index, row) in wire.observations.iter().enumerate() {
            row.checkpoint.validate()?;
            let same = row.checkpoint.identity == wire.node.identity;
            let valid = match &row.status {
                CandidateStatus::IdentityMismatch => !same,
                CandidateStatus::UnpinnedNode => same && wire.node.pins.is_empty(),
                CandidateStatus::MissingPinnedCheckpoints { missing } => {
                    references = references
                        .checked_add(missing.len())
                        .ok_or(InvalidObservation::PinBudget)?;
                    same && !missing.is_empty()
                        && canonical_pins(missing)
                        && missing
                            .iter()
                            .all(|pin| wire.node.pins.binary_search(pin).is_ok())
                        && row.checkpoint.sequence >= (wire.node.pins.len() - missing.len()) as u64
                        && row
                            .checkpoint
                            .publication
                            .as_ref()
                            .is_none_or(|head| missing.binary_search(head).is_err())
                        && (row.checkpoint.publication.is_some() || missing == &wire.node.pins)
                }
                CandidateStatus::SatisfiesPinnedCheckpoints => {
                    matching_rows += 1;
                    same && !wire.node.pins.is_empty()
                        && row.checkpoint.publication.is_some()
                        && row.checkpoint.sequence >= wire.node.pins.len() as u64
                }
            };
            if !valid {
                return Err(InvalidObservation::Candidate { index });
            }
            if matches!(&row.status, CandidateStatus::SatisfiesPinnedCheckpoints) {
                matching_facts.insert(row);
            } else {
                nonmatching_facts.insert(row);
            }
            if references > MAX_DISPLAY_PIN_REFERENCES {
                return Err(InvalidObservation::PinBudget);
            }
        }
        let count = wire.observations.len();
        let matching = match wire.status {
            BatchStatus::NotObserved if count == 0 && wire.physical_candidates == 0 => 0,
            BatchStatus::IncompleteObservation {
                matching_candidates,
            } if count > 0 && matching_candidates < 2 => matching_candidates,
            BatchStatus::Ambiguous {
                matching_candidates,
            } if matching_candidates >= 2 => matching_candidates,
            _ => return Err(InvalidObservation::Batch),
        };
        if wire.physical_candidates > count
            || (count > 0 && wire.physical_candidates == 0)
            || matching > wire.physical_candidates
            || matching > matching_rows
            || (matching_rows > 0 && matching == 0)
            || wire.physical_candidates > matching + (count - matching_rows)
            || matching_facts.len() > matching
            || nonmatching_facts.len() > wire.physical_candidates - matching
        {
            return Err(InvalidObservation::Batch);
        }
        Ok(Self {
            protocol: wire.protocol,
            node: wire.node,
            observations: wire.observations,
            physical_candidates: wire.physical_candidates,
            status: wire.status,
        })
    }
}

#[derive(Debug, error_definition::Error)]
pub enum DecodeObservationError {
    #[error("independent observation exceeds the response byte budget")]
    ResponseBudget,
    #[error("invalid independent observation JSON: {0}")]
    JsonBoundary(#[source] crate::Error),
    #[error("invalid independent observation fields: {0}")]
    Fields(#[source] json_codec::Error),
}

/// Decode a successful observation body, never an HTTP error or authentication
/// decision. The HTTP/UI layer must handle session expiry before view staleness.
pub fn decode_observation(
    bytes: &[u8],
) -> Result<IndependentSourceObservation, DecodeObservationError> {
    if bytes.len() > MAX_OBSERVATION_BYTES {
        return Err(DecodeObservationError::ResponseBudget);
    }
    let value = crate::strict_json(bytes).map_err(DecodeObservationError::JsonBoundary)?;
    json_codec::from_value(value).map_err(DecodeObservationError::Fields)
}

#[cfg(test)]
#[path = "../../../test/contract/unit/storage_independent_source.rs"]
mod tests;
