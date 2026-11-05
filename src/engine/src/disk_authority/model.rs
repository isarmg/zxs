use super::{Error, Result, codec};
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::{collections::BTreeMap, fmt, str::FromStr};

pub const MAX_OBJECTS: usize = 4096;
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PUBLICATIONS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Digest(pub(crate) [u8; 32]);
impl Digest {
    pub fn body(bytes: &[u8]) -> Self {
        codec::address("body", bytes)
    }
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}
impl FromStr for Digest {
    type Err = Error;
    fn from_str(text: &str) -> Result<Self> {
        if text.len() != 64
            || !text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid(
                "digest must be 64 lowercase hexadecimal digits",
            ));
        }
        let mut bytes = [0; 32];
        for (index, pair) in text.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            let digit = |b: u8| {
                if b.is_ascii_digit() {
                    b - b'0'
                } else {
                    b - b'a' + 10
                }
            };
            bytes[index] = digit(pair[0]) * 16 + digit(pair[1]);
        }
        Ok(Self(bytes))
    }
}
impl Serialize for Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}
impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(data_serialization::de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serialization(try_from = "String", into = "String")]
pub struct Token(String);
impl Token {
    pub fn new(text: impl Into<String>) -> Result<Self> {
        let text = text.into();
        if text.is_empty()
            || text.len() > 96
            || !text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(Error::Invalid("identifier is not a bounded ASCII token"));
        }
        Ok(Self(text))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for Token {
    type Error = Error;
    fn try_from(text: String) -> Result<Self> {
        Self::new(text)
    }
}
impl From<Token> for String {
    fn from(token: Token) -> Self {
        token.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum Role {
    Primary,
    Secondary,
    Tertiary,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Origin {
    pub pool_id: Token,
    pub disk_id: Token,
    pub slot_id: Token,
    pub binding_generation: u64,
}
impl Origin {
    fn validate(&self) -> Result<()> {
        if self.binding_generation == 0 {
            return Err(Error::Invalid("binding generation must be nonzero"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Identity {
    pub origin: Origin,
    pub incarnation: Digest,
    pub role: Role,
}
impl Identity {
    pub fn validate(&self) -> Result<()> {
        self.origin.validate()
    }
    pub fn digest(&self) -> Result<Digest> {
        self.validate()?;
        Ok(codec::address("identity", &codec::encode(self)?))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Format {
    #[serialization(rename = "independent-disk-journal")]
    Independent,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Body {
    pub digest: Digest,
    pub bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Commit {
    pub owner: Identity,
    pub history_id: Token,
    pub file_id: Token,
    pub parents: Vec<Digest>,
    pub body: Body,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SourceCheckpoint {
    pub identity: Identity,
    pub publication: Digest,
}

/// A primary's wire variant has no source/upstream/downstream field. Backup
/// variants hold only their selected source proofs, never downstream members.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "role", deny_unknown_fields, rename_all = "snake_case")]
pub enum Authority {
    Primary { heads: Vec<Digest> },
    Secondary { sources: Vec<SourceCheckpoint> },
    Tertiary { sources: Vec<SourceCheckpoint> },
}
impl Authority {
    pub fn role(&self) -> Role {
        match self {
            Self::Primary { .. } => Role::Primary,
            Self::Secondary { .. } => Role::Secondary,
            Self::Tertiary { .. } => Role::Tertiary,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Manifest {
    pub format: Format,
    pub identity: Identity,
    pub sequence: u64,
    pub authority: Authority,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct PublicationIntent {
    pub format: Format,
    pub operation_id: Token,
    pub target: Identity,
    pub base: Option<Digest>,
    pub manifest: Digest,
    pub request_digest: Digest,
    pub execution_binding: Digest,
    pub required_bodies: BTreeMap<Digest, Body>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct PreparedEvidence {
    pub format: Format,
    pub intent: Digest,
    pub manifest: Digest,
    pub request_digest: Digest,
    pub required_bodies: BTreeMap<Digest, Body>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Publication {
    pub format: Format,
    pub identity: Identity,
    pub sequence: u64,
    pub previous: Option<Digest>,
    pub operation_id: Token,
    pub request_digest: Digest,
    pub intent: Digest,
    pub prepared: Digest,
    pub manifest: Digest,
}

/// A verified durable decision in the independent disk protocol. Not an
/// online namespace/admission receipt or an offline capsule receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiskPublicationReceipt {
    pub(crate) identity: Identity,
    pub(crate) operation_id: Token,
    pub(crate) request_digest: Digest,
    pub(crate) sequence: u64,
    pub(crate) publication: Digest,
    pub(crate) manifest: Digest,
}
impl DiskPublicationReceipt {
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    pub fn operation_id(&self) -> &Token {
        &self.operation_id
    }
    pub fn request_digest(&self) -> Digest {
        self.request_digest
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn publication(&self) -> Digest {
        self.publication
    }
    pub fn manifest(&self) -> Digest {
        self.manifest
    }
}

impl Publication {
    pub(crate) fn receipt(&self, publication: Digest) -> DiskPublicationReceipt {
        DiskPublicationReceipt {
            identity: self.identity.clone(),
            operation_id: self.operation_id.clone(),
            request_digest: self.request_digest,
            sequence: self.sequence,
            publication,
            manifest: self.manifest,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(
    tag = "kind",
    content = "value",
    deny_unknown_fields,
    rename_all = "snake_case"
)]
pub(crate) enum Object {
    Commit(Commit),
    Manifest(Manifest),
    Intent(PublicationIntent),
    Prepared(PreparedEvidence),
    Publication(Publication),
}
impl Object {
    pub(crate) fn bytes(&self) -> Result<Vec<u8>> {
        codec::encode(self)
    }
    pub(crate) fn digest(&self) -> Result<Digest> {
        Ok(codec::address("object", &self.bytes()?))
    }
}

/// Bounded exact bytes, independent of the original machine's path. A verifier
/// returns only the reachable proof closure; callers cannot mutate that proof.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Bundle {
    pub(crate) objects: BTreeMap<Digest, Vec<u8>>,
    /// One independent FULL file per immutable commit/version, not content hash.
    pub(crate) bodies: BTreeMap<Digest, Vec<u8>>,
}
impl Bundle {
    pub fn add_commit(&mut self, commit: Commit, body: Option<&[u8]>) -> Result<Digest> {
        commit.owner.validate()?;
        if commit.owner.role != Role::Primary {
            return Err(Error::Invalid("only a primary owns history"));
        }
        if commit.parents.len() > MAX_OBJECTS || body.is_some_and(|bytes| bytes.len() > MAX_BYTES) {
            return Err(Error::Budget);
        }
        if let Some(bytes) = body
            && (Digest::body(bytes) != commit.body.digest
                || bytes.len() as u64 != commit.body.bytes)
        {
            return Err(Error::Invalid("body differs from declared content"));
        }
        let object = Object::Commit(commit);
        let bytes = object.bytes()?;
        let hash = object.digest()?;
        let added_object = check_entry(&self.objects, hash, &bytes)?;
        let added_body = match body {
            Some(bytes) => check_entry(&self.bodies, hash, bytes)?,
            None => 0,
        };
        self.admit(
            usize::from(!self.objects.contains_key(&hash)),
            added_object.checked_add(added_body).ok_or(Error::Budget)?,
        )?;
        if let Some(body) = body {
            self.bodies.entry(hash).or_insert_with(|| body.to_vec());
        }
        self.objects.entry(hash).or_insert(bytes);
        Ok(hash)
    }
    pub fn extend(&mut self, other: &Self) -> Result<()> {
        let mut added = 0usize;
        for (map, incoming) in [
            (&self.objects, &other.objects),
            (&self.bodies, &other.bodies),
        ] {
            for (hash, bytes) in incoming {
                added = added
                    .checked_add(check_entry(map, *hash, bytes)?)
                    .ok_or(Error::Budget)?;
            }
        }
        let count = other
            .objects
            .keys()
            .filter(|key| !self.objects.contains_key(key))
            .count();
        self.admit(count, added)?;
        self.objects.extend(other.objects.clone());
        self.bodies.extend(other.bodies.clone());
        Ok(())
    }
    pub(crate) fn insert(&mut self, object: Object) -> Result<Digest> {
        let bytes = object.bytes()?;
        let hash = object.digest()?;
        self.admit(
            usize::from(!self.objects.contains_key(&hash)),
            check_entry(&self.objects, hash, &bytes)?,
        )?;
        self.objects.entry(hash).or_insert(bytes);
        Ok(hash)
    }
    fn admit(&self, added_objects: usize, added_bytes: usize) -> Result<()> {
        let current = self
            .objects
            .values()
            .chain(self.bodies.values())
            .map(Vec::len)
            .sum::<usize>();
        if self
            .objects
            .len()
            .checked_add(added_objects)
            .is_none_or(|n| n > MAX_OBJECTS)
            || current
                .checked_add(added_bytes)
                .is_none_or(|n| n > MAX_BYTES)
        {
            return Err(Error::Budget);
        }
        Ok(())
    }
}
fn check_entry(map: &BTreeMap<Digest, Vec<u8>>, hash: Digest, bytes: &[u8]) -> Result<usize> {
    match map.get(&hash) {
        Some(existing) if existing != bytes => Err(Error::Corrupt(hash.to_string())),
        Some(_) => Ok(0),
        None => Ok(bytes.len()),
    }
}
