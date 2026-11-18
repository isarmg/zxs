//! One container's working state, separate from immutable content versions.
//!
//! These are durable logical records. Decoding them does not admit a container,
//! select a branch or authorize access to the referenced draft body.
use crate::{BranchId, ContentHash, FileNodeId, Generation, GenerationExhausted, Id, VersionId};
use data_serialization::{Deserialize, Serialize};
use std::fmt;

pub mod manifest;
mod incremental;
pub use incremental::IncrementalManifest;
mod unborn;
pub use unborn::{BirthRequest, UnbornBody, UnbornSaveRequest, UnbornWorking};
mod selection;
pub use selection::{WorkingContentExpectation, WorkingExpectation, WorkingSelection};

/// A selected occurrence, not a FileId-only or global-default lookup.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DraftLocation {
    pub repository: crate::RepositoryId,
    pub path: name_policy::PortablePath,
    pub entry: crate::EntryId,
    pub file: crate::FileId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum DraftCommand {
    SetMode {
        target: DraftLocation,
        expected: WorkingRevision,
        mode: CommitMode,
    },
    Write {
        target: DraftLocation,
        expected: WorkingRevision,
        base: DraftBase,
        content_hash: ContentHash,
        #[serialization(with = "crate::u64_string")]
        logical_size: u64,
    },
    Discard {
        target: DraftLocation,
        expected: WorkingRevision,
    },
    Commit {
        target: DraftLocation,
        expected: WorkingRevision,
        base: DraftBase,
        message: String,
    },
}

impl DraftCommand {
    pub const fn target(&self) -> &DraftLocation {
        match self {
            Self::SetMode { target, .. } | Self::Write { target, .. }
            | Self::Discard { target, .. } | Self::Commit { target, .. } => target,
        }
    }

    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SetMode { .. } => "file.working.mode",
            Self::Write { .. } => "file.draft.write",
            Self::Discard { .. } => "file.draft.discard",
            Self::Commit { .. } => "file.draft.commit",
        }
    }
}

pub mod kind {
    pub enum DraftPayload {}

    impl crate::IdKind for DraftPayload {
        const PREFIX: &'static str = "draft";
    }
}

/// A mutable working body's identity is never an immutable VersionId.
pub type DraftPayloadId = Id<kind::DraftPayload>;

/// Fences every change of commit mode or of the shared working body.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serialization(transparent)]
pub struct WorkingRevision(Generation);

impl WorkingRevision {
    pub const fn new(value: Generation) -> Self {
        Self(value)
    }

    pub const fn generation(self) -> Generation {
        self.0
    }

    pub fn next(self) -> Result<Self, GenerationExhausted> {
        self.0.next().map(Self)
    }
}

impl fmt::Display for WorkingRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum CommitMode {
    Automatic,
    Manual,
}

/// The selection captured by the first write to a clean shared draft.
/// Later writes retain this record instead of selecting today's branch HEAD.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DraftBase {
    branch_id: BranchId,
    node_id: FileNodeId,
    version_id: VersionId,
    selection_generation: Generation,
}

impl DraftBase {
    pub const fn new(
        branch_id: BranchId,
        node_id: FileNodeId,
        version_id: VersionId,
        selection_generation: Generation,
    ) -> Self {
        Self {
            branch_id,
            node_id,
            version_id,
            selection_generation,
        }
    }

    pub const fn branch_id(self) -> BranchId {
        self.branch_id
    }

    pub const fn node_id(self) -> FileNodeId {
        self.node_id
    }

    pub const fn version_id(self) -> VersionId {
        self.version_id
    }

    pub const fn selection_generation(self) -> Generation {
        self.selection_generation
    }
}

/// A saved body's stable identity. Full uploads retain their existing wire
/// commitment; range saves name the authenticated manifest, never a body hash.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(untagged, deny_unknown_fields)]
pub enum DraftPayload {
    Full {
        payload_id: DraftPayloadId,
        content_hash: ContentHash,
        #[serialization(with = "crate::u64_string")]
        logical_size: u64,
    },
    Ranges {
        payload_id: DraftPayloadId,
        manifest_commitment: manifest::ManifestCommitment,
        #[serialization(with = "crate::u64_string")]
        logical_size: u64,
    },
}

impl DraftPayload {
    pub const fn new(payload_id: DraftPayloadId, content_hash: ContentHash, logical_size: u64)
        -> Self
    {
        Self::Full { payload_id, content_hash, logical_size }
    }

    pub const fn ranges(
        payload_id: DraftPayloadId, manifest_commitment: manifest::ManifestCommitment,
        logical_size: u64,
    ) -> Self {
        Self::Ranges { payload_id, manifest_commitment, logical_size }
    }

    pub const fn payload_id(self) -> DraftPayloadId {
        match self {
            Self::Full { payload_id, .. } | Self::Ranges { payload_id, .. } => payload_id,
        }
    }

    pub const fn content_hash(self) -> Option<ContentHash> {
        match self {
            Self::Full { content_hash, .. } => Some(content_hash),
            Self::Ranges { .. } => None,
        }
    }

    pub const fn manifest_commitment(self) -> Option<manifest::ManifestCommitment> {
        match self {
            Self::Full { .. } => None,
            Self::Ranges { manifest_commitment, .. } => Some(manifest_commitment),
        }
    }

    pub const fn logical_size(self) -> u64 {
        match self {
            Self::Full { logical_size, .. } | Self::Ranges { logical_size, .. } => logical_size,
        }
    }
}

/// This mandatory record belongs inside the authoritative container record.
/// There is no default that could turn absent draft metadata into clean state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileWorkingState {
    /// The current container exists, but no historical FILE node does. This is
    /// not ManualClean: there is no selected Version to reopen or discard to.
    Unborn {
        working: UnbornWorking,
    },
    Automatic {
        revision: WorkingRevision,
    },
    ManualClean {
        revision: WorkingRevision,
    },
    ManualIncremental {
        revision: WorkingRevision,
        base: DraftBase,
        payload: DraftPayload,
        manifest: IncrementalManifest,
    },
    ManualDirty {
        revision: WorkingRevision,
        base: DraftBase,
        payload: DraftPayload,
    },
}

impl FileWorkingState {
    /// Actual still-referenced working storage, separate from historical FULL
    /// versions and from a sparse body's potentially much larger visible length.
    pub fn material_bytes(&self) -> u64 {
        match self {
            Self::Unborn { working } => match working.body() {
                UnbornBody::Empty {} => 0,
                UnbornBody::Saved { manifest } => manifest.allocated_bytes(),
            },
            Self::ManualDirty { payload, .. } => payload.logical_size(),
            Self::ManualIncremental { manifest, .. } => manifest.material().allocated_bytes(),
            Self::Automatic { .. } | Self::ManualClean { .. } => 0,
        }
    }

    pub const fn revision(&self) -> WorkingRevision {
        match self {
            Self::Unborn { working } => working.revision(),
            Self::Automatic { revision }
            | Self::ManualClean { revision }
            | Self::ManualDirty { revision, .. }
            | Self::ManualIncremental { revision, .. } => *revision,
        }
    }

    pub const fn mode(&self) -> CommitMode {
        match self {
            Self::Unborn { working } => working.mode(),
            Self::Automatic { .. } => CommitMode::Automatic,
            Self::ManualClean { .. } | Self::ManualDirty { .. }
            | Self::ManualIncremental { .. } => CommitMode::Manual,
        }
    }

    pub const fn draft(&self) -> Option<(&DraftBase, &DraftPayload)> {
        match self {
            Self::Unborn { .. } => None,
            Self::Automatic { .. } | Self::ManualClean { .. } => None,
            Self::ManualDirty { base, payload, .. }
            | Self::ManualIncremental { base, payload, .. } => Some((base, payload)),
        }
    }

    pub const fn incremental(&self) -> Option<&IncrementalManifest> {
        match self {
            Self::ManualIncremental { manifest, .. } => Some(manifest),
            Self::Unborn { .. } | Self::Automatic { .. } | Self::ManualClean { .. }
            | Self::ManualDirty { .. } => None,
        }
    }

    pub const fn unborn(&self) -> Option<&UnbornWorking> {
        match self {
            Self::Unborn { working } => Some(working),
            Self::Automatic { .. } | Self::ManualClean { .. } | Self::ManualDirty { .. }
            | Self::ManualIncremental { .. } => None,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/file_draft.rs"]
mod tests;
