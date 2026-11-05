use super::*;

/// A foreign-source evidence/review digest, distinct from a file body's hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForeignEvidenceDigest([u8; 32]);
impl ForeignEvidenceDigest {
    pub(super) fn from_digest(value: blake3::Hash) -> Self {
        Self(*value.as_bytes())
    }
}
impl std::fmt::Display for ForeignEvidenceDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(blake3::Hash::from_bytes(self.0).to_hex().as_str())
    }
}
impl std::str::FromStr for ForeignEvidenceDigest {
    type Err = storage_contract::ParseContentHashError;
    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        // Reuse the strict 64-lowercase-hex parser, without exposing the parsed
        // value as a body commitment or accepting a body in an import API.
        let value: ContentHash = value.parse()?;
        Ok(Self::from_digest(value.digest()))
    }
}
impl data_serialization::Serialize for ForeignEvidenceDigest {
    fn serialize<S: data_serialization::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// Hard-bounded inspection budgets. Exceeding a budget rejects the preview,
/// rather than silently returning a subset of the retained histories.
#[derive(Clone, Copy, Debug)]
pub struct ForeignImportLimits {
    pub(super) metadata_bytes: usize,
    pub(super) journal_bytes: usize,
    pub(super) versions: usize,
    pub(super) payload_bytes: u64,
}
impl Default for ForeignImportLimits {
    fn default() -> Self {
        Self {
            metadata_bytes: 256 * 1024 * 1024,
            journal_bytes: 64 * 1024 * 1024,
            versions: 100_000,
            payload_bytes: 1 << 40,
        }
    }
}
impl ForeignImportLimits {
    pub fn new(
        metadata_bytes: usize,
        journal_bytes: usize,
        versions: usize,
        payload_bytes: u64,
    ) -> Result<Self> {
        if metadata_bytes == 0
            || metadata_bytes > 512 * 1024 * 1024
            || journal_bytes == 0
            || journal_bytes > 128 * 1024 * 1024
            || versions == 0
            || versions > 100_000
            || payload_bytes > 1 << 40
        {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "invalid foreign preview budget",
            ));
        }
        Ok(Self {
            metadata_bytes,
            journal_bytes,
            versions,
            payload_bytes,
        })
    }
}

/// Original logical identity. Physical device/inode values are intentionally
/// absent; the original unmodified seal is retained separately by the preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignSourceIdentity {
    pub(super) registration: DiskRegistrationIdentity,
    pub(super) seal_digest: ForeignEvidenceDigest,
    pub(super) role_evidence: DiskRoleEvidence,
}
impl ForeignSourceIdentity {
    pub fn registration(&self) -> &DiskRegistrationIdentity {
        &self.registration
    }
    pub fn seal_digest(&self) -> &ForeignEvidenceDigest {
        &self.seal_digest
    }
    pub fn role_evidence(&self) -> &DiskRoleEvidence {
        &self.role_evidence
    }
}

/// This process's observation of the held directory, not a portable identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignLocalBinding {
    pub(super) device: u64,
    pub(super) inode: u64,
}
impl ForeignLocalBinding {
    pub fn device(&self) -> u64 {
        self.device
    }
    pub fn inode(&self) -> u64 {
        self.inode
    }
}

/// The last complete publication observed in this local journal. It makes no
/// claim to be the globally latest publication of the disconnected source pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignCheckpoint {
    pub(super) candidate_hash: String,
    pub(super) publication_hash: String,
    pub(super) sequence: u64,
    pub(super) authority_hash: Option<String>,
    pub(super) unpublished_tail: bool,
}
impl ForeignCheckpoint {
    pub fn candidate_hash(&self) -> &str {
        &self.candidate_hash
    }
    pub fn publication_hash(&self) -> &str {
        &self.publication_hash
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn authority_hash(&self) -> Option<&str> {
        self.authority_hash.as_deref()
    }
    pub fn has_unpublished_tail(&self) -> bool {
        self.unpublished_tail
    }
}

/// Exact primary identity from the version's original frozen placement and
/// publication receipt, not the current meaning of a similarly named slot.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ForeignPrimaryOrigin {
    pub(super) pool_id: String,
    pub(super) disk_id: String,
    pub(super) slot_id: String,
    pub(super) binding_generation: u64,
}
impl ForeignPrimaryOrigin {
    pub fn pool_id(&self) -> &str {
        &self.pool_id
    }
    pub fn disk_id(&self) -> &str {
        &self.disk_id
    }
    pub fn slot_id(&self) -> &str {
        &self.slot_id
    }
    pub fn binding_generation(&self) -> u64 {
        self.binding_generation
    }
}

/// Availability is an observation alongside the unchanged logical content
/// disposition. A missing body is never converted to a persistent tombstone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForeignVersionAvailability {
    VerifiedLocal,
    TemporarilyUnavailable,
    RecordedPurged,
    RecordedMigrated,
}

pub struct ForeignHistory {
    pub(super) group_id: String,
    pub(super) origin: ForeignPrimaryOrigin,
    pub(super) manifest_hash: String,
    pub(super) manifest: HistoryManifest,
    pub(super) availability: BTreeMap<String, ForeignVersionAvailability>,
    pub(super) origin_publications: BTreeMap<String, String>,
}
impl ForeignHistory {
    pub fn group_id(&self) -> &str {
        &self.group_id
    }
    pub fn origin(&self) -> &ForeignPrimaryOrigin {
        &self.origin
    }
    pub fn manifest_hash(&self) -> &str {
        &self.manifest_hash
    }
    pub fn manifest(&self) -> &HistoryManifest {
        &self.manifest
    }
    pub fn availability(&self) -> &BTreeMap<String, ForeignVersionAvailability> {
        &self.availability
    }
    pub fn origin_publications(&self) -> &BTreeMap<String, String> {
        &self.origin_publications
    }
}

pub struct ForeignImportPreview {
    pub(super) root: DiskRoot,
    pub(super) seal_bytes: Vec<u8>,
    pub(super) journal_path: PathBuf,
    pub(super) journal_digest: ForeignEvidenceDigest,
    pub(super) authority_anchors: BTreeMap<PathBuf, ForeignEvidenceDigest>,
    pub(super) limits: ForeignImportLimits,
    pub(super) source: ForeignSourceIdentity,
    pub(super) local: ForeignLocalBinding,
    pub(super) source_level: u32,
    pub(super) checkpoint: ForeignCheckpoint,
    pub(super) histories: Vec<ForeignHistory>,
    pub(super) digest: ForeignEvidenceDigest,
}
impl ForeignImportPreview {
    pub fn source(&self) -> &ForeignSourceIdentity {
        &self.source
    }
    pub fn local_binding(&self) -> &ForeignLocalBinding {
        &self.local
    }
    pub fn source_level(&self) -> u32 {
        self.source_level
    }
    pub fn checkpoint(&self) -> &ForeignCheckpoint {
        &self.checkpoint
    }
    pub fn histories(&self) -> &[ForeignHistory] {
        &self.histories
    }
    pub fn digest(&self) -> &ForeignEvidenceDigest {
        &self.digest
    }
    /// The original evidence remains byte-for-byte intact, including its old
    /// physical identity. It is not a new seal for the current local directory.
    pub fn original_seal(&self) -> &[u8] {
        &self.seal_bytes
    }
}

pub struct ForeignMappedHistory {
    pub(super) group_id: String,
    pub(super) history_tree_id: String,
    pub(super) path: name_policy::PortablePath,
}
impl ForeignMappedHistory {
    pub fn group_id(&self) -> &str {
        &self.group_id
    }
    pub fn history_tree_id(&self) -> &str {
        &self.history_tree_id
    }
    pub fn path(&self) -> &name_policy::PortablePath {
        &self.path
    }
}

/// A read-only review binding. There is deliberately no execute/import method.
pub struct ForeignImportMapping {
    pub(super) source_digest: ForeignEvidenceDigest,
    pub(super) digest: ForeignEvidenceDigest,
    pub(super) target: DiskRegistrationIdentity,
    pub(super) target_sequence: u64,
    pub(super) histories: Vec<ForeignMappedHistory>,
}
impl ForeignImportMapping {
    pub fn source_digest(&self) -> &ForeignEvidenceDigest {
        &self.source_digest
    }
    pub fn digest(&self) -> &ForeignEvidenceDigest {
        &self.digest
    }
    pub fn target(&self) -> &DiskRegistrationIdentity {
        &self.target
    }
    pub fn target_sequence(&self) -> u64 {
        self.target_sequence
    }
    pub fn histories(&self) -> &[ForeignMappedHistory] {
        &self.histories
    }
}
